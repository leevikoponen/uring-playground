use std::any::Any;

use io_uring::{cqueue, squeue};

use super::wrapper::MapOutput;
use crate::batch::{Link2, Single};

/// Abstract representation of a singular `io_uring` operation.
///
/// # Safety
///
/// Implementations must ensure that parameters like mutable buffers are
/// kept alive for the entire duration of the operation, like through
/// yielding them from [`Operation::take_required_allocations`].
#[must_use]
pub unsafe trait Operation: Unpin {
    /// What this operation ultimately produces.
    type Output: Unpin;

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
        Self: Operation + Sized,
    {
        MapOutput::new(self, function)
    }

    /// Convert the operation into a batch.
    fn into_batch(self) -> Single<Self>
    where
        Self: Oneshot + Sized,
    {
        Single::new(self)
    }

    /// Link the operation to another.
    fn link_with<T>(self, another: T) -> Link2<Self, T>
    where
        Self: Oneshot + Sized,
        T: Oneshot,
    {
        Link2::new(self, another)
    }
}

/// Operation that guarantees to only produce one completion entry.
///
/// # Safety
///
/// The guarantee must actually hold.
#[must_use]
pub unsafe trait Oneshot: Operation {}
