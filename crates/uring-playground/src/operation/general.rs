use std::{
    any::Any,
    io::{Error, Result},
    os::fd::RawFd,
    pin::Pin,
};

use io_uring::{
    cqueue,
    opcode,
    squeue,
    types::{CancelBuilder, Fd, Fixed},
};
use nix::poll::PollFlags;

use crate::{
    driver::{FileIndex, OperationId, UringFd},
    operation::Operation,
};

/// Wait for a file's readiness.
///
/// Corresponds to [`io_uring_prep_poll_add(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_poll_add.3.html).
#[must_use]
pub struct PollAdd {
    file: UringFd,
    flags: PollFlags,
    multi: bool,
}

impl PollAdd {
    pub const fn new(file: UringFd, flags: PollFlags) -> Self {
        Self {
            file,
            flags,
            multi: false,
        }
    }

    /// Choose whether to keep receiving completions for readiness.
    pub const fn with_multi(mut self, multi: bool) -> Self {
        self.multi = multi;
        self
    }
}

// SAFETY: no parameters to invalidate
unsafe impl Operation for PollAdd {
    type Output = Result<PollFlags>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        let flags = self
            .flags
            .bits()
            .try_into()
            .expect("poll flags should fit into their parameter slot");

        match self.file {
            UringFd::Fd(file) => opcode::PollAdd::new(Fd(file), flags),
            UringFd::Fixed(file) => opcode::PollAdd::new(Fixed(file), flags),
        }
        .multi(self.multi)
        .build()
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        if entry.result().is_negative() {
            return Err(Error::from_raw_os_error(-entry.result()));
        }

        let ready = entry
            .result()
            .try_into()
            .ok()
            .and_then(PollFlags::from_bits)
            .expect("poll completion should produce valid readiness indication");

        Ok(ready)
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        None
    }
}

/// Remove interest about a file's readiness.
///
/// Corresponds to [`io_uring_prep_poll_remove(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_poll_remove.3.html).
#[must_use]
pub struct PollRemove {
    operation: OperationId,
}

impl PollRemove {
    pub const fn new(operation: OperationId) -> Self {
        Self { operation }
    }
}

// SAFETY: no parameters to invalidate
unsafe impl Operation for PollRemove {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::PollRemove::new(self.operation.user_data_bits()).build()
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

/// Get a traditional file descriptor for a fixed file.
///
/// Corresponds to [`io_uring_prep_fixed_fd_install(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_fixed_fd_install.3.html).
#[must_use]
pub struct FixedFdInstall {
    index: FileIndex,
}

impl FixedFdInstall {
    pub const fn new(index: FileIndex) -> Self {
        Self { index }
    }
}

// SAFETY: parameters are safe to invalidate after submit
unsafe impl Operation for FixedFdInstall {
    type Output = Result<RawFd>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::FixedFdInstall::new(Fixed(self.index), 0).build()
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        if entry.result().is_negative() {
            return Err(Error::from_raw_os_error(-entry.result()));
        }

        Ok(entry.result())
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        None
    }
}

/// Close a file descriptor.
///
/// Corresponds to [`io_uring_prep_close(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_close.3.html).
#[must_use]
pub struct Close {
    file: UringFd,
}

impl Close {
    pub const fn new(file: UringFd) -> Self {
        Self { file }
    }
}

// SAFETY: no parameters to invalidate
unsafe impl Operation for Close {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        match self.file {
            UringFd::Fd(descriptor) => opcode::Close::new(Fd(descriptor)),
            UringFd::Fixed(descriptor) => opcode::Close::new(Fixed(descriptor)),
        }
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

/// Cancel other operations.
///
/// Corresponds to [`io_uring_prep_cancel(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_cancel.3.html).
#[must_use]
pub struct Cancel {
    builder: Option<CancelBuilder>,
}

impl Cancel {
    /// Cancel operations with the given user data value.
    pub const fn with_matching_user_data(id: OperationId) -> Self {
        Self {
            builder: Some(CancelBuilder::user_data(id.user_data_bits())),
        }
    }

    /// Cancel operations using the given file descriptor.
    pub fn using_file(value: UringFd) -> Self {
        match value {
            UringFd::Fd(value) => Self {
                builder: Some(CancelBuilder::fd(Fd(value))),
            },
            UringFd::Fixed(value) => Self {
                builder: Some(CancelBuilder::fd(Fixed(value))),
            },
        }
    }

    /// Cancel all matching operations instead of just the first one.
    pub fn all_matching(mut self) -> Self {
        self.builder = self.builder.map(CancelBuilder::all);
        self
    }
}

// SAFETY: no parameters to invalidate
unsafe impl Operation for Cancel {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        let builder = self
            .get_mut()
            .builder
            .take()
            .expect("cancel operation should have been created with criteria");

        opcode::AsyncCancel2::new(builder).build()
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
