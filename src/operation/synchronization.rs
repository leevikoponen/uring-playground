use std::{any::Any, sync::atomic::AtomicU32};

use io_uring::{cqueue, opcode, squeue};

use crate::operation::{Oneshot, Operation};

/// Invoke a futex wait request.
///
/// Corresponds to [io_uring_prep_futex_wait(3)](https://www.man7.org/linux/man-pages/man3/io_uring_prep_futex_wait.3.html).
#[must_use]
pub struct FutexWait<'futex> {
    futex: &'futex AtomicU32,
    compare: u32,
}

impl<'futex> FutexWait<'futex> {
    pub const fn new(futex: &'futex AtomicU32, compare: u32) -> Self {
        Self { futex, compare }
    }
}

// SAFETY: futex pointer is inconsequential for safety
unsafe impl Operation for FutexWait<'_> {
    type Output = ();

    fn build_submission(&mut self) -> squeue::Entry {
        opcode::FutexWait::new(self.futex.as_ptr().cast_const(), self.compare.into(), 0, 0).build()
    }

    unsafe fn handle_completion(&mut self, _: cqueue::Entry) -> Self::Output {}

    fn take_required_allocations(&mut self) -> Option<Box<dyn Any>> {
        None
    }
}

// SAFETY: only returns once
unsafe impl Oneshot for FutexWait<'_> {}

/// Invoke a futex wake request.
///
/// Corresponds to [io_uring_prep_futex_wake(3)](https://www.man7.org/linux/man-pages/man3/io_uring_prep_futex_wake.3.html).
#[must_use]
pub struct FutexWake<'futex> {
    futex: &'futex AtomicU32,
    count: u32,
}

impl<'futex> FutexWake<'futex> {
    pub const fn new(futex: &'futex AtomicU32, count: u32) -> Self {
        Self { futex, count }
    }
}

// SAFETY: futex pointer is inconsequential for safety
unsafe impl Operation for FutexWake<'_> {
    type Output = ();

    fn build_submission(&mut self) -> squeue::Entry {
        opcode::FutexWake::new(self.futex.as_ptr().cast_const(), self.count.into(), 0, 0).build()
    }

    unsafe fn handle_completion(&mut self, _: cqueue::Entry) -> Self::Output {}

    fn take_required_allocations(&mut self) -> Option<Box<dyn Any>> {
        None
    }
}

// SAFETY: only returns once
unsafe impl Oneshot for FutexWake<'_> {}
