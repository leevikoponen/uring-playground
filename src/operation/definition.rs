use std::{
    any::Any,
    cell::RefCell,
    task::{Context, Poll},
};

use io_uring::{cqueue, squeue};

use crate::{
    operation::{Link2, MapOutput, Single, SubmitAndWait},
    reactor::Reactor,
};

/// Abstract representation of a singular `io_uring` operation.
///
/// # Safety
///
/// Implementations must ensure that parameters like mutable buffers are
/// kept alive for the entire duration of the operation, like through
/// yielding them from [`Operation::take_required_allocations`].
#[must_use]
pub unsafe trait Operation {
    /// What this operation ultimately produces.
    type Output;

    /// Build a submission that represents this operation.
    #[must_use]
    fn build_submission(&mut self) -> squeue::Entry;

    /// Process this operation's completion.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the entry corresponds to the submission
    /// from [`Operation::build_submission`].
    #[must_use]
    unsafe fn handle_completion(&mut self, entry: cqueue::Entry) -> Self::Output;

    /// Take away allocated values that have to live for the duration of the
    /// operation instead of just until the submission has been made.
    ///
    /// This allows the operation struct to be safely dropped as long as the
    /// caller ensures these values get stashed somewhere.
    #[must_use]
    fn take_required_allocations(&mut self) -> Option<Box<dyn Any>>;

    /// Transform the output to another type.
    fn map_output<F>(self, function: F) -> MapOutput<Self, F>
    where
        Self: Sized,
    {
        MapOutput::new(self, function)
    }
}

/// Operation that guarantees to only produce one completion entry.
///
/// # Safety
///
/// The guarantee must actually hold.
#[must_use]
pub unsafe trait Oneshot: Operation {
    /// Convert the operation into a batch.
    fn into_batch(self) -> Single<Self>
    where
        Self: Sized,
    {
        Single::new(self)
    }

    /// Link the operation to another.
    fn link_with<T>(self, another: T) -> Link2<Self, T>
    where
        Self: Sized,
        T: Oneshot,
    {
        Link2::new(self, another)
    }
}

/// Abstract representation of multiple oneshot operations.
///
/// # Safety
///
/// The same requirements as for operations apply.
#[must_use]
pub unsafe trait Batch {
    /// Handle to store information about submitted entries.
    ///
    /// This should allow for working with more than just one entry, such as
    /// with linked operations.
    type Handle;

    /// What this operation ultimately produces.
    type Output;

    /// Submit entries onto the specified reactor.
    #[must_use]
    fn submit_entries(&mut self, reactor: &mut Reactor, context: Option<&Context>) -> Self::Handle;

    /// Poll for progress on the operations.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the handle originates from calling
    /// [`Batch::submit_entries`].
    unsafe fn poll_progress(
        &mut self,
        handle: Self::Handle,
        reactor: &mut Reactor,
        context: &Context,
    ) -> Poll<Self::Output>;

    /// Mark operations as ignored as a cancellation step in case of drop.
    fn drop_operations(&mut self, handle: Self::Handle, reactor: &mut Reactor);

    /// Create a submission future.
    fn build_submission(self, reactor: &RefCell<Reactor>) -> SubmitAndWait<'_, Self>
    where
        Self: Batch + Sized,
    {
        SubmitAndWait::new(reactor, self)
    }
}
