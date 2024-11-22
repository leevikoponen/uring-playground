//! Primary abstraction around operations and some wrappers.
use std::any::Any;

use io_uring::{cqueue, squeue};

/// Abstract representation of a singular `io_uring` operation.
///
/// # Safety
///
/// Implementations must ensure that parameters like mutable buffers are kept
/// alive for the entire duration of the operation, like through yielding them
/// from [`Operation::take_required_allocations`].
pub unsafe trait Operation: Unpin {
    /// What this operation ultimately produces.
    type Output: Unpin;

    /// Build a submission that represents this operation.
    fn build_submission(&mut self) -> squeue::Entry;

    /// Process this operation's completion.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the entry corresponds to the submission from
    /// [`Operation::build_submission`].
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

/// Wrapper for [`Oneshot`] that captures the output internally.
pub struct StashOutput<O: Oneshot> {
    operation: O,
    output: Option<O::Output>,
}

impl<O: Oneshot> StashOutput<O> {
    pub const fn new(operation: O) -> Self {
        Self {
            operation,
            output: None,
        }
    }

    pub const fn not_finished(&self) -> bool {
        self.output.is_none()
    }

    #[must_use]
    pub fn take_output(&mut self) -> Option<O::Output> {
        self.output.take()
    }
}

// SAFETY: the internal operation promises safety
unsafe impl<O: Oneshot> Operation for StashOutput<O> {
    type Output = ();

    fn build_submission(&mut self) -> squeue::Entry {
        self.operation.build_submission()
    }

    unsafe fn handle_completion(&mut self, entry: cqueue::Entry) -> Self::Output {
        assert!(self.output.is_none());

        // SAFETY: we control the submission
        unsafe {
            self.output = Some(self.operation.handle_completion(entry));
        }
    }

    fn take_required_allocations(&mut self) -> Option<Box<dyn Any>> {
        if self.output.is_some() {
            return None;
        }

        self.operation.take_required_allocations()
    }
}

// SAFETY: the internal operation is oneshot
unsafe impl<O: Oneshot> Oneshot for StashOutput<O> {}
