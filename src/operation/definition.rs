use std::any::Any;

use io_uring::{cqueue, squeue};

/// Abstract representation of a singular `io_uring` operation.
///
/// # Safety
///
/// Implementations must ensure that parameters like mutable buffers are
/// kept alive for the entire duration of the operation, like through
/// yielding them from [`Operation::take_required_allocations`].
pub unsafe trait Operation: Unpin {
    /// What this operation ultimately produces.
    type Output: Unpin;

    /// Build a submission that represents this operation.
    fn build_submission(&mut self) -> squeue::Entry;

    /// Process this operation's completion.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the entry corresponds to the submission
    /// from [`Operation::build_submission`].
    unsafe fn handle_completion(&mut self, entry: cqueue::Entry) -> Self::Output;

    /// Take away allocated values that have to live for the duration of the
    /// operation instead of just until the submission has been made.
    ///
    /// This allows the operation struct to be safely dropped as long as the
    /// caller ensures these values get stashed somewhere.
    #[must_use = "required allocations must be stashed away somewhere"]
    fn take_required_allocations(&mut self) -> Option<Box<dyn Any>>;
}

/// [`Operation`] that guarantees to only produce one completion entry.
///
/// # Safety
///
/// The same requirements as [`Operation`] apply.
pub unsafe trait Oneshot: Operation {}
