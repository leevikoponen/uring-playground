//! Bits for submitting tasks and polling for their completion or blocking until
//! there's progress available as needed.
use std::{
    any::Any,
    collections::VecDeque,
    io::Result,
    task::{Context, Poll, Waker},
    time::Duration,
};

use fnv::FnvBuildHasher;
use indexmap::IndexMap;
use io_uring::{cqueue, squeue, types::SubmitArgs, IoUring};
use thunderdome::{Arena, Index};

/// Strongly typed handle to a submitted operation.
///
/// We're using a generational arena to ensure that it's practically impossible
/// for a reused index to be confused with some rogue operation pointing at the
/// same slot.
#[derive(Debug, Clone, Copy)]
#[must_use]
pub struct OperationId(Index);

/// The tracked state of an operation that's stored by the reactor.
enum OperationState {
    /// The operation has been submitted and a caller task is waiting to be
    /// notified of progress.
    Waiting(Waker),
    /// The operation has completed and the waiting task has been notified.
    ///
    /// This value will be passed away on the next call to
    /// [`Reactor::poll_completion`].
    Completed(cqueue::Entry),
    /// The operation is producing completions at a faster rate than they're
    /// consumed.
    Buffering(VecDeque<cqueue::Entry>),
    /// Nobody is waiting for the operation.
    ///
    /// Either the operation was created without passing a [`Waker`] or
    /// [`Reactor::ignore_operation`] has been called.
    Ignored(Option<Box<dyn Any>>),
}

/// Reactor for submitting and waiting for operations.
///
/// # Parameter safety
///
/// We internally queue up operations instead of pushing them to the submission
/// queue immediately in order to make use of `IORING_FEAT_SUBMIT_STABLE`, which
/// allows us to assume that all operations that don't have mutable buffers
/// (which must outlast the entire operation lifecycle) can safely be dropped
/// as long as their drop implementation ensures the registered entry doesn't
/// ever actually get submitted to the kernel.
///
/// This is done as we aren't able to remove submitted entries from the real
/// queue shared with the kernel.
///
/// # Reactor mutability
///
/// This time I'm choosing to just surface the fact that we need interior
/// mutability one way or another and the methods just take mutable references
/// instead of hiding this detail.
///
/// This is required as a reference to the reactor will necessarily be stored in
/// multiple futures, despite knowing that only one will get polled at a time in
/// the single threaded context that `io_uring` already necessitates.
///
/// Theoretically [`Context::ext`] could be used for this and get injected
/// inside some kind of `block_on` style call, but I don't want to depend on
/// unstable features.
#[must_use]
pub struct Reactor {
    ring: IoUring,
    tracked: Arena<OperationState>,
    unsubmitted: IndexMap<Index, squeue::Entry, FnvBuildHasher>,
}

impl Reactor {
    /// Initialize the reactor with the specified queue size.
    ///
    /// This queue size is also used to preallocate storage for internal state.
    ///
    /// # Errors
    ///
    /// If initializing the internal `io_uring` instance fails.
    pub fn new(entries: u32) -> Result<Self> {
        let capacity = entries.try_into().unwrap_or(usize::MAX);
        let ring = IoUring::builder()
            .setup_coop_taskrun()
            .setup_single_issuer()
            .build(entries)?;

        Ok(Self {
            ring,
            tracked: Arena::with_capacity(capacity),
            unsubmitted: IndexMap::with_capacity_and_hasher(capacity, FnvBuildHasher::default()),
        })
    }

    /// Queue an operation to get submitted and return an unique identifier to
    /// it's internal state.
    ///
    /// This function only optionally takes in a [`Context`] in order to allow
    /// that an operation isn't initially waited by anything, as the poll
    /// implementation will also update the waker regardless.
    ///
    /// # Safety
    ///
    /// The caller must ensure that any parameters are valid and will be kept
    /// valid according to the kernel's requirements.
    pub unsafe fn queue_submission(
        &mut self,
        entry: squeue::Entry,
        context: Option<&Context>,
    ) -> OperationId {
        let initial = context
            .map(Context::waker)
            .cloned()
            .map_or(OperationState::Ignored(None), OperationState::Waiting);

        let index = self.tracked.insert(initial);
        self.unsubmitted
            .insert(index, entry.user_data(index.to_bits()));

        OperationId(index)
    }

    /// Poll for an operation's completion.
    ///
    /// # Panics
    ///
    /// If the specified operation doesn't exist or an internal sanity check
    /// assertion fails.
    pub fn poll_completion(
        &mut self,
        OperationId(index): OperationId,
        context: &Context,
    ) -> Poll<cqueue::Entry> {
        match &mut self.tracked[index] {
            OperationState::Waiting(waker) => {
                if !waker.will_wake(context.waker()) {
                    context.waker().clone_into(waker);
                }

                Poll::Pending
            }
            OperationState::Completed(entry) => {
                let output = if cqueue::more(entry.flags()) {
                    std::mem::replace(
                        &mut self.tracked[index],
                        OperationState::Waiting(context.waker().clone()),
                    )
                } else {
                    self.tracked.remove(index).unwrap()
                };

                let OperationState::Completed(entry) = output else {
                    unreachable!();
                };

                Poll::Ready(entry)
            }
            OperationState::Buffering(entries) => {
                let Some(entry) = entries.pop_front() else {
                    return Poll::Pending;
                };

                if !entries.is_empty() {
                    context.waker().wake_by_ref();
                    return Poll::Ready(entry);
                }

                if !cqueue::more(entry.flags()) {
                    self.tracked.remove(index).unwrap();
                    return Poll::Ready(entry);
                }

                self.tracked[index] = OperationState::Waiting(context.waker().clone());
                Poll::Ready(entry)
            }
            OperationState::Ignored(data) => {
                assert!(
                    data.is_none(),
                    "an explicitly forgotten operation shouldn't be polled again"
                );

                self.tracked[index] = OperationState::Waiting(context.waker().clone());
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
    pub fn ignore_operation(
        &mut self,
        OperationId(index): OperationId,
        data: Option<Box<dyn Any>>,
    ) {
        if self.unsubmitted.shift_remove(&index).is_some() {
            self.tracked.remove(index).unwrap();
            return;
        }

        match std::mem::replace(&mut self.tracked[index], OperationState::Ignored(data)) {
            OperationState::Waiting(_) | OperationState::Ignored(_) => (),
            OperationState::Completed(entry) if cqueue::more(entry.flags()) => (),
            OperationState::Completed(_) => _ = self.tracked.remove(index).unwrap(),
            OperationState::Buffering(entries) => match entries.into_iter().last() {
                Some(entry) if cqueue::more(entry.flags()) => (),
                Some(_) => _ = self.tracked.remove(index).unwrap(),
                None => (),
            },
        }
    }

    /// Submit queued entries and wait until tracked operations progress or the
    /// provided timeout elapses.
    ///
    /// # Errors
    ///
    /// If the submission queue is full.
    ///
    /// # Panics
    ///
    /// If a completion isn't associated with a tracked operation or an internal
    /// sanity check assertion fails.
    pub fn wait_for_progress(&mut self, timeout: Option<Duration>) -> Result<()> {
        let (submitter, mut submission, completion) = self.ring.split();

        let pushed = self
            .unsubmitted
            .drain(..)
            .try_fold(0, |count, (_, entry)| {
                // SAFETY: initial inserter guaranteed validity
                while unsafe { submission.push(&entry) }.is_err() {
                    // try to free up some space for the remaining entries, but we're probably
                    // screwed here in the error case if there are any linked operations, honestly
                    // no idea what could be done to recover regardless of how we implement things
                    // and where the entries are actually pushed
                    submitter.squeue_wait()?;
                }

                Ok(count + 1) as Result<usize>
            })?;

        // uncertain what the ideal logic should be, but we definitely want to block if
        // we haven't managed to submit operations and nothing has completed
        let wanted = usize::from(pushed == 0 && completion.is_empty());
        let result = timeout.map_or_else(
            || submitter.submit_and_wait(wanted),
            |timeout| {
                submitter.submit_with_args(wanted, &SubmitArgs::new().timespec(&timeout.into()))
            },
        );

        match result {
            Ok(_) => (),
            Err(error) if error.raw_os_error() == Some(libc::ETIME) => assert!(timeout.is_some()),
            Err(error) => return Err(error),
        }

        for entry in completion {
            let index = Index::from_bits(entry.user_data()).unwrap();

            match &mut self.tracked[index] {
                state @ OperationState::Waiting(_) => {
                    let previous = std::mem::replace(state, OperationState::Completed(entry));

                    let OperationState::Waiting(waker) = previous else {
                        unreachable!();
                    };

                    waker.wake();
                }
                state @ OperationState::Completed(_) => {
                    let replacement = OperationState::Buffering(VecDeque::with_capacity(2));
                    let previous = std::mem::replace(state, replacement);

                    let OperationState::Completed(previous) = previous else {
                        unreachable!();
                    };

                    let OperationState::Buffering(entries) = state else {
                        unreachable!();
                    };

                    entries.push_back(previous);
                    entries.push_back(entry);
                }
                OperationState::Buffering(entries) => entries.push_back(entry),
                OperationState::Ignored(_) if cqueue::more(entry.flags()) => (),
                OperationState::Ignored(_) => _ = self.tracked.remove(index).unwrap(),
            }
        }

        Ok(())
    }
}
