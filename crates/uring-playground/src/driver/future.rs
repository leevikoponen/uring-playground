use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use futures_lite::Stream;
use pin_project::{pin_project, pinned_drop};

use crate::{
    driver::{OperationId, Reactor},
    operation::{Batch, Cancel, Operation},
};

/// Future for submitting and waiting for an operation to complete.
#[pin_project(PinnedDrop)]
#[must_use]
pub struct Oneshot<'reactor, O: Operation> {
    reactor: &'reactor Reactor,
    #[pin]
    operation: O,
    handle: Option<OperationId>,
}

impl<'reactor, O: Operation> Oneshot<'reactor, O> {
    /// Create an oneshot completion future.
    pub const fn new(reactor: &'reactor Reactor, operation: O) -> Self {
        Self {
            reactor,
            operation,
            handle: None,
        }
    }

    /// Attach a cancellation trigger to this submission future.
    pub const fn with_cancellation<F>(self, trigger: F) -> Cancellable<'reactor, Self, F> {
        Cancellable {
            inner: self,
            state: Cancellation::Untriggered(trigger),
        }
    }
}

impl<O: Operation> Future for Oneshot<'_, O> {
    type Output = O::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let mut this = self.project();

        // SAFETY: implementation guarantees validity
        let handle = *this.handle.get_or_insert_with(|| unsafe {
            this.reactor
                .enqueue_submission(this.operation.as_mut().build_submission(), Some(cx))
        });

        this.reactor
            .poll_completion(handle, cx)
            // SAFETY: we control the submission above
            .map(|output| unsafe {
                *this.handle = None;
                this.operation.handle_completion(output)
            })
    }
}

#[pinned_drop]
impl<O: Operation> PinnedDrop for Oneshot<'_, O> {
    fn drop(self: Pin<&mut Self>) {
        let this = self.project();

        if let Some(handle) = this.handle.take() {
            this.reactor
                .ignore_operation(handle, this.operation.take_allocations());
        }
    }
}

/// Stream for submitting and waiting for a operation with multiple
/// completions.
#[pin_project(PinnedDrop)]
#[must_use]
pub struct Multishot<'reactor, O: Operation> {
    reactor: &'reactor Reactor,
    #[pin]
    operation: O,
    handle: Option<OperationId>,
    done: bool,
}

impl<'reactor, O: Operation> Multishot<'reactor, O> {
    /// Create a multishot completion stream.
    pub const fn new(reactor: &'reactor Reactor, operation: O) -> Self {
        Self {
            reactor,
            operation,
            handle: None,
            done: false,
        }
    }

    /// Attach a cancellation trigger to this submission future.
    pub const fn with_cancellation<F>(self, trigger: F) -> Cancellable<'reactor, Self, F> {
        Cancellable {
            inner: self,
            state: Cancellation::Untriggered(trigger),
        }
    }
}

impl<O: Operation> Stream for Multishot<'_, O> {
    type Item = O::Output;

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.done {
            return (0, Some(0));
        }

        (1, None)
    }

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }

        let mut this = self.project();

        // SAFETY: implementation guarantees safety
        let handle = *this.handle.get_or_insert_with(|| unsafe {
            this.reactor
                .enqueue_submission(this.operation.as_mut().build_submission(), Some(cx))
        });

        this.reactor.poll_completion(handle, cx).map(|entry| {
            if !io_uring::cqueue::more(entry.flags()) {
                *this.handle = None;
                *this.done = true;
            }

            // SAFETY: we control the submission above
            Some(unsafe { this.operation.handle_completion(entry) })
        })
    }
}

#[pinned_drop]
impl<O: Operation> PinnedDrop for Multishot<'_, O> {
    fn drop(self: Pin<&mut Self>) {
        let this = self.project();

        if let Some(handle) = this.handle.take() {
            this.reactor
                .ignore_operation(handle, this.operation.take_allocations());
        }
    }
}

/// Future for submitting and waiting for a batch of operations to complete.
#[pin_project(PinnedDrop)]
#[must_use]
pub struct Linked<'reactor, B: Batch> {
    reactor: &'reactor Reactor,
    #[pin]
    batch: B,
    handle: Option<B::Handle>,
}

impl<'reactor, B: Batch> Linked<'reactor, B> {
    /// Create a new submission future.
    pub const fn new(reactor: &'reactor Reactor, batch: B) -> Self {
        Self {
            reactor,
            batch,
            handle: None,
        }
    }

    /// Attach a cancellation trigger to this submission future.
    pub const fn with_cancellation<F>(self, trigger: F) -> Cancellable<'reactor, Self, F> {
        Cancellable {
            inner: self,
            state: Cancellation::Untriggered(trigger),
        }
    }
}

impl<B: Batch> Future for Linked<'_, B> {
    type Output = B::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let mut this = self.project();
        let handle = *this
            .handle
            .get_or_insert_with(|| this.batch.as_mut().submit_entries(this.reactor, Some(cx)));

        // SAFETY: we control the submission above
        unsafe { this.batch.as_mut().poll_progress(handle, this.reactor, cx) }.map(|output| {
            *this.handle = None;
            output
        })
    }
}

#[pinned_drop]
impl<B: Batch> PinnedDrop for Linked<'_, B> {
    fn drop(self: Pin<&mut Self>) {
        let this = self.project();

        if let Some(handle) = this.handle.take() {
            this.batch.drop_operations(handle, this.reactor);
        }
    }
}

/// Internal state of handling cancellation support by reacting to being
/// triggered and then driving an cancellation operations as needed.
#[pin_project(project = CancellationProjection)]
enum Cancellation<'reactor, T> {
    Untriggered(#[pin] T),
    Ongoing(#[pin] Oneshot<'reactor, Cancel>),
    Done,
}

impl<'reactor, T: Future<Output = ()>> Cancellation<'reactor, T> {
    /// Poll for possible progress on the cancellation.
    ///
    /// # Notes
    ///
    /// It's worth realizing that there's nothing reasonable to do when
    /// cancellation fails, so we just swallow that error case here.
    fn maybe_progress(
        mut self: Pin<&mut Self>,
        context: &mut Context,
        reactor: &'reactor Reactor,
        operation: OperationId,
    ) -> Poll<()> {
        loop {
            match self.as_mut().project() {
                CancellationProjection::Untriggered(trigger) => {
                    if trigger.poll(context).is_pending() {
                        return Poll::Ready(());
                    }

                    self.set(Cancellation::Ongoing(
                        Cancel::with_matching_user_data(operation).build_oneshot(reactor),
                    ));
                }
                CancellationProjection::Ongoing(cancellation) => {
                    return cancellation
                        .poll(context)
                        .map(|_| self.set(Cancellation::Done));
                }
                CancellationProjection::Done => return Poll::Ready(()),
            }
        }
    }
}

/// Wrapper for driving operation futures with cancellation.
///
/// # Details
///
/// An asynchronous cancellation is started if the given trigger future
/// completes, resulting in getting interrupted or no longer producing output.
///
/// The cancellation is interrupted if the operation actually completes first,
/// other (unlikely) types of failures similarly result in the operation just
/// being driven to completion as usual.
#[pin_project]
#[must_use]
pub struct Cancellable<'reactor, F, T> {
    #[pin]
    inner: F,
    #[pin]
    state: Cancellation<'reactor, T>,
}

impl<'reactor, O: Operation, T: Future<Output = ()>> Future
    for Cancellable<'reactor, Oneshot<'reactor, O>, T>
{
    type Output = O::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let this = self.project();
        if let Some(operation) = this.inner.handle {
            std::task::ready!(this.state.maybe_progress(cx, this.inner.reactor, operation));
        }

        this.inner.poll(cx)
    }
}

impl<'reactor, O: Operation, T: Future<Output = ()>> Stream
    for Cancellable<'reactor, Multishot<'reactor, O>, T>
{
    type Item = O::Output;

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        let this = self.project();
        if let Some(operation) = this.inner.handle {
            std::task::ready!(this.state.maybe_progress(cx, this.inner.reactor, operation));
        }

        this.inner.poll_next(cx)
    }
}

impl<'reactor, B: Batch, T: Future<Output = ()>> Future
    for Cancellable<'reactor, Linked<'reactor, B>, T>
{
    type Output = B::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let this = self.project();
        if let Some(operations) = this.inner.handle {
            std::task::ready!(this.state.maybe_progress(
                cx,
                this.inner.reactor,
                B::first_operation(operations)
            ));
        }

        this.inner.poll(cx)
    }
}

#[cfg(test)]
mod test {
    use std::time::Duration;

    use futures_lite::{FutureExt as _, StreamExt as _, future};

    use crate::{
        driver::Reactor,
        operation::{Batch as _, Nop, Operation as _},
    };

    /// Helper macro to initialize a reactor and drive a test future with it.
    ///
    /// # Notes
    ///
    /// This is a bit unfortunate as just a function with an async closure
    /// ends up not playing ball with how the borrow checker models things.
    macro_rules! drive_with_reactor {
        (|$reactor:ident| $future:expr) => {
            let $reactor =
                Reactor::initialize(16, None).expect("should be able to initialize reactor");

            future::block_on($future.or(async {
                let completed = $reactor
                    .wait_for_progress(1, Some(Duration::from_secs(1)), None)
                    .expect("should be able to wait for progress");

                assert!(
                    completed >= 1,
                    "test operations should complete before timeout"
                );

                future::yield_now().await;
                unreachable!("driving simple tests should only need one tick");
            }));
        };
    }

    #[test]
    fn can_submit_operation() {
        drive_with_reactor!(|reactor| async {
            Nop::new()
                .build_oneshot(&reactor)
                .await
                .expect("nop operation should have completed successfully");
        });
    }

    #[test]
    fn streams_work_as_expected() {
        drive_with_reactor!(|reactor| async {
            let mut stream = std::pin::pin!(Nop::new().build_multishot(&reactor));

            assert!(
                stream.next().await.as_ref().is_some_and(Result::is_ok),
                "nop stream should produce completion on first poll"
            );

            assert!(
                future::poll_once(stream.next())
                    .await
                    .as_ref()
                    .is_some_and(Option::is_none),
                "nop stream should return none immediately on second poll"
            );
        });
    }

    #[test]
    fn mock_cancellation_functions_correctly() {
        drive_with_reactor!(|reactor| async {
            assert!(
                Nop::new()
                    .build_oneshot(&reactor)
                    .with_cancellation(future::pending())
                    .await
                    .as_ref()
                    .is_ok(),
                "fake cancellation source shouldn't trigger"
            );
        });
    }

    #[test]
    fn linked_operations_work_as_expected() {
        drive_with_reactor!(|reactor| async {
            let (first, second) = Nop::new()
                .link_with(Nop::new())
                .build_submission(&reactor)
                .await;

            first.and(second).expect("linked nops should both complete");
        });
    }
}
