use std::{
    any::Any,
    io::{Error, Result},
    os::fd::{AsRawFd as _, BorrowedFd},
};

use io_uring::{cqueue, opcode, squeue, types::Fd};

use crate::operation::{Oneshot, Operation};

/// Operation that reads from a file.
///
/// Corresponds to [io_uring_prep_read(3)](https://www.man7.org/linux/man-pages/man3/io_uring_prep_read.3.html).
#[must_use]
#[non_exhaustive]
pub struct Read<'file> {
    file: BorrowedFd<'file>,
    buffer: Vec<u8>,
}

impl<'file> Read<'file> {
    pub const fn new(file: BorrowedFd<'file>, buffer: Vec<u8>) -> Self {
        Self { file, buffer }
    }
}

// SAFETY: parameters are safe to invalidate after submit
unsafe impl Operation for Read<'_> {
    type Output = Result<Vec<u8>>;

    fn build_submission(&mut self) -> squeue::Entry {
        // SAFETY: correctly slicing into the uninitialized section
        let (pointer, length) = unsafe {
            let start = self.buffer.as_mut_ptr().add(self.buffer.len());
            let remaining = self.buffer.capacity() - self.buffer.len();

            (start, remaining)
        };

        opcode::Read::new(
            Fd(self.file.as_raw_fd()),
            pointer,
            length.try_into().unwrap_or(u32::MAX),
        )
        .build()
    }

    unsafe fn handle_completion(&mut self, entry: cqueue::Entry) -> Self::Output {
        if entry.result().is_negative() {
            return Err(Error::from_raw_os_error(-entry.result()));
        }

        // SAFETY: we have to trust the kernel
        unsafe {
            let amount = entry.result().try_into().unwrap_or(usize::MAX);
            self.buffer.set_len(self.buffer.len() + amount);
        }

        Ok(std::mem::take(&mut self.buffer))
    }

    fn take_required_allocations(&mut self) -> Option<Box<dyn Any>> {
        Some(Box::new(std::mem::take(&mut self.buffer)))
    }
}

// SAFETY: only returns once
unsafe impl Oneshot for Read<'_> {}

/// Operation that writes to a file.
///
/// Corresponds to [io_uring_prep_write(3)](https://www.man7.org/linux/man-pages/man3/io_uring_prep_write.3.html).
#[must_use]
#[non_exhaustive]
pub struct Write<'parameters> {
    file: BorrowedFd<'parameters>,
    buffer: &'parameters [u8],
}

impl<'parameters> Write<'parameters> {
    pub const fn new(file: BorrowedFd<'parameters>, buffer: &'parameters [u8]) -> Self {
        Self { file, buffer }
    }
}

// SAFETY: parameters are safe to invalidate after submit
unsafe impl Operation for Write<'_> {
    type Output = Result<usize>;

    fn build_submission(&mut self) -> squeue::Entry {
        opcode::Write::new(
            Fd(self.file.as_raw_fd()),
            self.buffer.as_ptr(),
            self.buffer.len().try_into().unwrap_or(u32::MAX),
        )
        .build()
    }

    unsafe fn handle_completion(&mut self, entry: cqueue::Entry) -> Self::Output {
        if entry.result().is_negative() {
            return Err(Error::from_raw_os_error(-entry.result()));
        }

        Ok(entry.result().try_into().unwrap_or(usize::MAX))
    }

    fn take_required_allocations(&mut self) -> Option<Box<dyn Any>> {
        None
    }
}

// SAFETY: only returns once
unsafe impl Oneshot for Write<'_> {}
