use std::{
    any::Any,
    cell::{Cell, RefCell},
    collections::VecDeque,
    io::{Error, ErrorKind, Result},
    os::fd::{AsRawFd, RawFd},
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};

use fnv::FnvBuildHasher;
use futures_lite::future;
use indexmap::IndexMap;
use io_uring::{
    IoUring,
    Submitter,
    cqueue,
    squeue,
    types::{CancelBuilder, SubmitArgs},
};
use local_async::WakeupQueue;
use slab::Slab;

/// Strongly typed handle to a submitted operation.
#[derive(Clone, Copy)]
#[must_use]
pub struct OperationId(u64);

impl OperationId {
    /// Construct from the given index.
    fn from_index(value: usize) -> Self {
        value
            .try_into()
            .map(Self)
            .expect("slab index shouldn't reasonably overflow user data")
    }

    /// Use the identifier as an index.
    fn into_index(self) -> usize {
        self.0
            .try_into()
            .expect("user data shouldn't reasonably overflow pointer size")
    }

    /// Get the raw value used as user data to associate submission and
    /// completion queue entries.
    #[must_use]
    pub const fn user_data_bits(self) -> u64 {
        self.0
    }
}

/// The tracked state of an operation that's stored by the reactor.
enum OperationLifecycle {
    /// The operation has been submitted and a caller task is waiting to be
    /// notified of progress.
    Waiting(Waker),
    /// The operation has completed and the waiting task has been notified.
    ///
    /// This value will be passed away on the next poll.
    Completed(cqueue::Entry),
    /// The operation is producing completions at a faster rate than they're
    /// consumed, which shouldn't be common, but must be handled gracefully.
    Buffering(VecDeque<cqueue::Entry>),
    /// Either the operation was created without passing a waker and hasn't been
    /// polled yet, or it has been explicitly ignored.
    Ignored(Option<Box<dyn Any + Send>>),
}

impl Default for OperationLifecycle {
    fn default() -> Self {
        Self::Ignored(None)
    }
}

/// The mutable state needed by the reactor.
struct ReactorState {
    ring: IoUring,
    queued: IndexMap<usize, squeue::Entry, FnvBuildHasher>,
    ongoing: Slab<OperationLifecycle>,
}

impl ReactorState {
    /// Handle all completion entries received from the kernel.
    ///
    /// # Panics
    ///
    /// If an operation isn't associated with valid internal state.
    fn handle_completions(&mut self) -> usize {
        self.ring.completion().fold(0, |count, entry| {
            let (index, state) = usize::try_from(entry.user_data())
                .ok()
                .and_then(|index| Some((index, self.ongoing.get_mut(index)?)))
                .expect("user data should have been a valid arena index");

            match state {
                OperationLifecycle::Waiting(_) => {
                    match std::mem::replace(state, OperationLifecycle::Completed(entry)) {
                        OperationLifecycle::Waiting(waker) => waker.wake(),
                        _ => unreachable!("state should have just been checked"),
                    }
                }
                OperationLifecycle::Completed(previous) => {
                    *state = OperationLifecycle::Buffering(VecDeque::from_iter([
                        previous.clone(),
                        entry,
                    ]));
                }
                OperationLifecycle::Buffering(entries) => entries.push_back(entry),
                OperationLifecycle::Ignored(_) if cqueue::more(entry.flags()) => (),
                OperationLifecycle::Ignored(_) => {
                    self.ongoing
                        .try_remove(index)
                        .expect("operation existence should have already been checked");
                }
            }

            count + 1
        })
    }

    /// Push operations into the real queue shared with the kernel.
    fn push_prepared(&mut self) -> Result<usize> {
        self.queued.drain(..).try_fold(0, |count, (_, entry)| {
            loop {
                // SAFETY: initial inserter guaranteed validity
                if unsafe { self.ring.submission().push(&entry) }.is_ok() {
                    return Ok(count + 1);
                }

                // try to free up some space for the remaining entries, but we're screwed here
                // in the error case if there are any linked operations, honestly no idea what
                // could be done to recover regardless of how we implement things and when/where
                // the entries are actually pushed
                self.ring
                    .submitter()
                    .squeue_wait()
                    .expect("should be able to recover from a full submission queue");

                // load the hopefully changed queue positions from the kernel
                self.ring.submission().sync();
            }
        })
    }
}

/// Reactor for submitting and waiting for operations.
///
/// # Notes
///
/// We internally queue up operations instead of pushing them to the submission
/// queue immediately in order to make use of `IORING_FEAT_SUBMIT_STABLE`, which
/// allows us to assume that all operations that don't have mutable buffers
/// (which must always outlast the entire operation lifecycle) can safely be
/// dropped as long as their drop implementation ensures the registered entry
/// doesn't ever actually get submitted to the kernel.
///
/// This has to be done as we aren't able to remove submitted entries from the
/// real queue shared with the kernel, actually upholding the stricter rules
/// for everything isn't ergonomic in a language like Rust that doesn't just
/// throw everything on a GC'd heap.
#[must_use]
pub struct Reactor {
    state: RefCell<ReactorState>,
    pub(super) buffer_group_id_alloc: Cell<u16>,
}

impl Reactor {
    /// Initialize the reactor with the specified queue size also utilized as
    /// preallocated storage for internal state.
    ///
    /// # Errors
    ///
    /// If initializing the internal `io_uring` instance fails.
    ///
    /// # Notes
    ///
    /// The parent parameter allows sharing kernel side worker pools with
    /// another instance, such as when running in a thread per core style
    /// scenario where total isolation between rings might not be necessary.
    pub fn initialize(entries: u32, parent: Option<RawFd>) -> Result<Self> {
        let capacity = entries.try_into().unwrap_or(usize::MAX);
        let mut builder = IoUring::builder();
        builder.setup_coop_taskrun();
        builder.setup_single_issuer();
        builder.setup_defer_taskrun();
        if let Some(reactor) = parent {
            builder.setup_attach_wq(reactor);
        }

        let ring = builder.build(entries)?;
        if !ring.params().is_feature_submit_stable() {
            // I'll expand a bit here to note that our implementation is completely
            // dependent on the fact that we can just drop unsubmitted events and queuing an
            // event only requires mutable buffers to be kept alive, which is much easier to
            // accomplish compared to requiring literally everything to kept alive even
            // after the kernel has transferred the request into it's internals,
            // accomplishing which would require the use of owned or reference counted
            // values for almost all parameters
            return Err(Error::new(
                ErrorKind::Unsupported,
                "IORING_FEAT_SUBMIT_STABLE is required for our safety requirements to make sense",
            ));
        }

        Ok(Self {
            state: RefCell::new(ReactorState {
                ring,
                queued: IndexMap::with_capacity_and_hasher(capacity, FnvBuildHasher::default()),
                ongoing: Slab::with_capacity(capacity),
            }),
            buffer_group_id_alloc: Cell::new(0),
        })
    }

    /// Run an operation using the ring instance.
    ///
    /// # Notes
    ///
    /// This isn't a great way do do things, but I don't really think wrapping
    /// every such operation is reasonable as they'll end up as slightly
    /// incomplete anyways due to this crate not containing the entire
    /// surrounding abstractions.
    pub fn use_inner<T>(&self, action: impl FnOnce(Submitter<'_>) -> T) -> T {
        action(self.state.borrow().ring.submitter())
    }

    /// Add an operation to get submitted and return an unique identifier to
    /// it's internal state.
    ///
    /// # Safety
    ///
    /// The caller must ensure that any parameters are valid and will be kept
    /// valid according to the kernel's requirements.
    ///
    /// # Notes
    ///
    /// This function only optionally takes in a [`Context`] in order to allow
    /// that an operation isn't initially waited by anything, as the poll
    /// implementation will also update the waker regardless.
    pub unsafe fn enqueue_submission(
        &self,
        entry: squeue::Entry,
        context: Option<&Context>,
    ) -> OperationId {
        let mut guard = self.state.borrow_mut();
        let handle = OperationId::from_index(
            guard.ongoing.insert(
                context
                    .map(Context::waker)
                    .cloned()
                    .map(OperationLifecycle::Waiting)
                    .unwrap_or_default(),
            ),
        );

        guard.queued.insert(
            handle.into_index(),
            entry.user_data(handle.user_data_bits()),
        );

        handle
    }

    /// Prepare operation state in order to wait for an externally invoked
    /// completion, such as from a ring message operation.
    ///
    /// The caller should utilize [`OperationId::user_data_bits`] when creating
    /// the operation that will end up waking this reactor.
    ///
    /// # Notes
    ///
    /// The semantics of the completion are entirely up to the side producing
    /// them with ring message operations.
    ///
    /// Injecting `IORING_CQE_F_MORE` might be a good idea if multishot like
    /// behaviour is used, but convineantly doing so is reliant on
    /// [tokio-rs/io-uring#371](https://github.com/tokio-rs/io-uring/pull/371).
    pub fn prepare_slot(&self, context: Option<&Context>) -> OperationId {
        let mut guard = self.state.borrow_mut();
        let inner = guard.ongoing.insert(
            context
                .map(Context::waker)
                .cloned()
                .map(OperationLifecycle::Waiting)
                .unwrap_or_default(),
        );

        OperationId::from_index(inner)
    }

    /// Poll for an operation's completion.
    ///
    /// # Panics
    ///
    /// If the specified operation doesn't exist or an internal sanity check
    /// assertion fails.
    ///
    /// # Notes
    ///
    /// This method will remove any operation state when processing the final
    /// completion, you shouldn't need to explicitly remove operations in the
    /// happy path.
    pub fn poll_completion(&self, handle: OperationId, context: &Context) -> Poll<cqueue::Entry> {
        let mut guard = self.state.borrow_mut();
        let slot = guard
            .ongoing
            .get_mut(handle.into_index())
            .expect("removed operations shouldn't be polled");

        match slot {
            OperationLifecycle::Waiting(current) => {
                if !current.will_wake(context.waker()) {
                    context.waker().clone_into(current);
                }

                Poll::Pending
            }
            OperationLifecycle::Completed(entry) => {
                let output = if cqueue::more(entry.flags()) {
                    std::mem::replace(slot, OperationLifecycle::Waiting(context.waker().clone()))
                } else {
                    guard
                        .ongoing
                        .try_remove(handle.into_index())
                        .expect("operation existence should have already been checked")
                };

                let OperationLifecycle::Completed(entry) = output else {
                    unreachable!("state should have already been checked");
                };

                Poll::Ready(entry)
            }
            OperationLifecycle::Buffering(entries) => {
                let Some(first) = entries.pop_front() else {
                    *slot = OperationLifecycle::Waiting(context.waker().clone());
                    return Poll::Pending;
                };

                if !entries.is_empty() {
                    context.waker().wake_by_ref();
                    return Poll::Ready(first);
                }

                if !cqueue::more(first.flags()) {
                    guard
                        .ongoing
                        .try_remove(handle.into_index())
                        .expect("buffered entry should have corresponding state");

                    return Poll::Ready(first);
                }

                *slot = OperationLifecycle::Waiting(context.waker().clone());
                Poll::Ready(first)
            }
            OperationLifecycle::Ignored(data) => {
                // we can't do anything here if there's an explicitly forgotten operation with
                // associated data, polling again would nesseciate losing the parameter
                // allocations given to us for keeping alive until the operation finishes
                assert!(
                    data.is_none(),
                    "an explicitly forgotten operation shouldn't be polled again"
                );

                *slot = OperationLifecycle::Waiting(context.waker().clone());
                Poll::Pending
            }
        }
    }

    /// Mark a submitted operation as ignored.
    ///
    /// The state parameter allows callers to uphold the safety requirements
    /// through handling the situation when the operation has already been
    /// submitted and the parameters must be kept alive.
    ///
    /// # Panics
    ///
    /// If the specified operation doesn't exist or an internal sanity check
    /// assertion fails.
    pub fn ignore_operation(&self, handle: OperationId, data: Option<Box<dyn Any + Send>>) {
        let mut guard = self.state.borrow_mut();

        // this might seem a little expensive, but this function shouldn't be called
        // particularly often and the actual O(N) code is happening on containers with
        // relatively small values, not that there should be a horribly large amount of
        // queued entries anyways as users should probably be capping away extreme
        // concurrency on a single thread somehow, with io_uring being designed around
        // thread per core after all
        if guard.queued.shift_remove(&handle.into_index()).is_some() {
            guard
                .ongoing
                .try_remove(handle.into_index())
                .expect("even unsubmitted operations should be attached to tracked state");

            return;
        }

        let slot = guard
            .ongoing
            .get_mut(handle.into_index())
            .expect("already removed operations shouldn't be ignored");

        match std::mem::replace(slot, OperationLifecycle::Ignored(data)) {
            OperationLifecycle::Waiting(_) | OperationLifecycle::Ignored(_) => return,
            OperationLifecycle::Completed(entry) if cqueue::more(entry.flags()) => return,
            OperationLifecycle::Completed(_) => (),
            OperationLifecycle::Buffering(entries) => match entries.into_iter().last() {
                Some(entry) if cqueue::more(entry.flags()) => return,
                Some(_) => (),
                None => return,
            },
        }

        guard
            .ongoing
            .try_remove(handle.into_index())
            .expect("the operation's existence should have been checked");
    }

    /// Synchronously wait for all ongoing operations to be cancelled.
    ///
    /// # Errors
    ///
    /// If the cancellation operation fails.
    pub fn cancel_all(&self) -> Result<()> {
        self.state
            .borrow()
            .ring
            .submitter()
            .register_sync_cancel(None, CancelBuilder::any())
            .or_else(|error| {
                if error.kind() == ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(error)
                }
            })
    }

    /// Submit queued entries and wait until tracked operations progress or the
    /// provided timeout elapses, optionally allowing a extra minimum timeout
    /// that can be waited for to allow for more completions to happen.
    ///
    /// # Errors
    ///
    /// If calling `io_uring_enter` fails internally.
    ///
    /// # Panics
    ///
    /// If a completion isn't associated with a tracked operation or an internal
    /// sanity check assertion fails.
    pub fn wait_for_progress(
        &self,
        wanted: usize,
        timeout: Option<Duration>,
        wait: Option<Duration>,
    ) -> Result<usize> {
        let mut guard = self.state.borrow_mut();
        guard.push_prepared()?;

        match guard.ring.submitter().submit_with_args(
            wanted,
            &SubmitArgs::new()
                .timespec(&timeout.unwrap_or(Duration::ZERO).into())
                .min_wait_usec(
                    wait.as_ref()
                        .map_or(0, Duration::as_millis)
                        .try_into()
                        .unwrap_or(u32::MAX),
                ),
        ) {
            Ok(amount) => {
                let handled = guard.handle_completions();

                assert_eq!(amount, handled, "should handle all completions");
                assert!(amount >= wanted, "should wait for requested completions");

                Ok(amount)
            }
            Err(error) => {
                if error.raw_os_error() != Some(libc::ETIME) {
                    return Err(error);
                }

                assert!(
                    timeout.is_some(),
                    "timeouts should happen only when requested"
                );

                Ok(0)
            }
        }
    }

    /// Run a future concurrently with waiting for progress.
    ///
    /// # Errors
    ///
    /// If waiting for progress fails.
    pub fn block_on<F: IntoFuture>(
        &self,
        timer: Option<WakeupQueue>,
        wait: Option<Duration>,
        future: F,
    ) -> Result<F::Output> {
        future::block_on(future::or(async { Ok(future.await) }, async {
            let Some(wakeups) = timer else {
                loop {
                    self.wait_for_progress(1, None, wait)?;
                    future::yield_now().await;
                }
            };

            loop {
                let Some(upcoming) = wakeups.next_scheduled() else {
                    self.wait_for_progress(1, None, wait)?;
                    future::yield_now().await;
                    continue;
                };

                let duration = upcoming.checked_duration_since(Instant::now());
                if duration.is_none() {
                    // there are wakeups that should have been triggered by now, so give them the
                    // possibility to affect things before actually waiting for progress
                    wakeups.trigger_elapsed(upcoming);
                    future::yield_now().await;
                }

                let completed = self.wait_for_progress(1, duration, wait)?;
                if completed == 0 {
                    // we always wait for completions so this must be the timeout case
                    wakeups.trigger_elapsed(upcoming);
                }

                future::yield_now().await;
            }
        }))
    }
}

impl AsRawFd for Reactor {
    fn as_raw_fd(&self) -> RawFd {
        self.state.borrow().ring.as_raw_fd()
    }
}

impl Drop for Reactor {
    fn drop(&mut self) {
        // I believe the kernel does understand that in flight operations can't continue
        // and pointers might have become invalidated when the io_uring instance gets
        // closed on drop, but we might as well still be a little more graceful than
        // strictly necessary for safety, this isn't exactly a hot path after all
        self.cancel_all()
            .expect("should be able to cancel leftover operations");
    }
}

#[cfg(test)]
mod test {
    use futures_lite::future;
    use io_uring::opcode::Nop;

    use crate::driver::Reactor;

    #[test]
    fn can_drive_operation() {
        let reactor = Reactor::initialize(16, None).expect("should be able to initialize reactor");

        // SAFETY: nothing to invalidate
        let id = unsafe { reactor.enqueue_submission(Nop::new().build(), None) };
        let output = future::block_on(future::poll_once(future::poll_fn(|context| {
            reactor.poll_completion(id, context)
        })));

        assert!(
            output.is_none(),
            "operation should not be complete immediately"
        );

        reactor
            .wait_for_progress(1, None, None)
            .expect("should be able to wait for progress");

        let output = future::block_on(future::poll_once(future::poll_fn(|context| {
            reactor.poll_completion(id, context)
        })));

        assert!(
            output.is_some_and(|entry| !entry.result().is_negative()),
            "nop operation should have completed successfully"
        );
    }
}
