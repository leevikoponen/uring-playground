use std::any::Any;

use io_uring::{cqueue, squeue};

use crate::operation::{Oneshot, Operation};

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