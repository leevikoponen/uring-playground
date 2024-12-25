use std::{
    any::Any,
    io::{Error, Result},
    pin::Pin,
    sync::atomic::AtomicU32,
};

use io_uring::{cqueue, opcode, squeue};

use crate::operation::Operation;

#[expect(
    clippy::as_conversions,
    clippy::cast_sign_loss,
    reason = "types of constants are quite arbitrary due to inconsistencies between syscalls"
)]
mod futex {
    pub const PRIVATE_FLAG: u32 = libc::FUTEX_PRIVATE_FLAG as _;
    pub const BITSET_MASK_ANY: u64 = libc::FUTEX_BITSET_MATCH_ANY as _;
}

/// Invoke a futex wait request.
///
/// Corresponds to [`io_uring_prep_futex_wait(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_futex_wait.3.html).
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
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::FutexWait::new(
            self.futex.as_ptr().cast_const(),
            self.compare.into(),
            futex::BITSET_MASK_ANY,
            futex::PRIVATE_FLAG,
        )
        .build()
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        if entry.result().is_negative() {
            return Err(Error::from_raw_os_error(-entry.result()));
        }

        Ok(())
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        None
    }
}

/// Invoke a futex wake request.
///
/// Corresponds to [`io_uring_prep_futex_wake(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_futex_wake.3.html).
#[must_use]
pub struct FutexWake<'futex> {
    futex: &'futex AtomicU32,
    count: u64,
}

impl<'futex> FutexWake<'futex> {
    pub const fn new(futex: &'futex AtomicU32, count: u64) -> Self {
        Self { futex, count }
    }
}

// SAFETY: futex pointer is inconsequential for safety
unsafe impl Operation for FutexWake<'_> {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::FutexWake::new(
            self.futex.as_ptr().cast_const(),
            self.count,
            futex::BITSET_MASK_ANY,
            futex::PRIVATE_FLAG,
        )
        .build()
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        if entry.result().is_negative() {
            return Err(Error::from_raw_os_error(-entry.result()));
        }

        Ok(())
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        None
    }
}
