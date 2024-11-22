use std::any::Any;

use io_uring::{cqueue, opcode, squeue};

use crate::operation::{Oneshot, Operation};

/// Operation that does nothing.
///
/// Corresponds to [io_uring_prep_nop(3)](https://www.man7.org/linux/man-pages/man3/io_uring_prep_nop.3.html).
// TODO: support fault injection
#[must_use]
#[non_exhaustive]
pub struct Nop;

impl Nop {
    #[expect(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {}
    }
}

// SAFETY: no parameters to invalidate
unsafe impl Operation for Nop {
    type Output = ();

    fn build_submission(&mut self) -> squeue::Entry {
        opcode::Nop::new().build()
    }

    unsafe fn handle_completion(&mut self, _: cqueue::Entry) -> Self::Output {}

    fn take_required_allocations(&mut self) -> Option<Box<dyn Any>> {
        None
    }
}

// SAFETY: only returns once
unsafe impl Oneshot for Nop {}
