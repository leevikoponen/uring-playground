use std::{
    any::Any,
    ffi::CStr,
    io::{Error, Result},
    marker::PhantomData,
    os::fd::RawFd,
    pin::Pin,
};

use io_uring::{
    cqueue,
    opcode,
    squeue,
    types::{DestinationSlot, Fd, Fixed},
};
use nix::{
    fcntl::{AtFlags, OFlag, OpenHow, RenameFlags},
    sys::stat::Mode,
    unistd::{LinkatFlags, UnlinkatFlags},
};

use crate::{
    driver::{FileIndex, Location, UringFd},
    operation::Operation,
};

/// Get information about a file.
///
/// Corresponds to [`io_uring_prep_statx(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_statx.3.html).
#[must_use]
pub struct Stat<'parameters> {
    location: Location<'parameters>,
    buffer: Option<Box<libc::statx>>,
}

impl<'parameters> Stat<'parameters> {
    pub fn new(location: Location<'parameters>) -> Self {
        Self {
            location,
            // SAFETY: zeroed state is perfectly valid for statx buffer
            buffer: Some(Box::new(unsafe { std::mem::zeroed() })),
        }
    }
}

// SAFETY: the buffer is passed on correctly
unsafe impl Operation for Stat<'_> {
    type Output = Result<libc::statx>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        let this = self.get_mut();
        let buffer = this
            .buffer
            .as_deref_mut()
            .map(std::ptr::from_mut)
            .expect("buffer should exist until completion")
            .cast();

        let path = this.location.path_ptr();

        opcode::Statx::new(this.location.parent_fd(), path, buffer).build()
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        if entry.result().is_negative() {
            return Err(Error::from_raw_os_error(-entry.result()));
        }

        let output = *self
            .get_mut()
            .buffer
            .take()
            .expect("buffer should exist until completion");

        Ok(output)
    }

    #[expect(
        clippy::as_conversions,
        reason = "just specifying to allocate a dynamic object"
    )]
    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        self.get_mut()
            .buffer
            .take()
            .map(|allocation| Box::new(allocation) as Box<dyn Any + Send>)
    }
}

/// Open a file.
///
/// Corresponds to [`io_uring_prep_openat2(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_openat2.3.html).
#[must_use]
pub struct Open<'parameters, T> {
    location: Location<'parameters>,
    how: OpenHow,
    marker: PhantomData<T>,
}

impl<'parameters> Open<'parameters, RawFd> {
    pub fn new(location: Location<'parameters>, mode: Mode, flags: OFlag) -> Self {
        Self {
            location,
            how: OpenHow::new().mode(mode).flags(flags),
            marker: PhantomData,
        }
    }

    /// Utilize a fixed file descriptor instead.
    pub const fn use_fixed(self) -> Open<'parameters, FileIndex> {
        Open {
            location: self.location,
            how: self.how,
            marker: PhantomData,
        }
    }
}

// SAFETY: parameters are safe to invalidate after submit
unsafe impl Operation for Open<'_, RawFd> {
    type Output = Result<RawFd>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::OpenAt2::new(
            self.location.parent_fd(),
            self.location.path_ptr(),
            std::ptr::from_ref(&self.how).cast(),
        )
        .build()
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

// SAFETY: parameters are safe to invalidate after submit
unsafe impl Operation for Open<'_, FileIndex> {
    type Output = Result<FileIndex>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::OpenAt2::new(
            self.location.parent_fd(),
            self.location.path_ptr(),
            std::ptr::from_ref(&self.how).cast(),
        )
        .file_index(Some(DestinationSlot::auto_target()))
        .build()
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        entry
            .result()
            .try_into()
            .map_err(|_| Error::from_raw_os_error(-entry.result()))
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        None
    }
}

/// Create a directory.
///
/// Corresponds to [`io_uring_prep_mkdirat(3)`](https://man7.org/linux/man-pages/man3/io_uring_prep_mkdirat.3.html).
#[must_use]
pub struct MkDir<'parameters> {
    location: Location<'parameters>,
    mode: Mode,
}

impl<'parameters> MkDir<'parameters> {
    pub const fn new(location: Location<'parameters>, mode: Mode) -> Self {
        Self { location, mode }
    }
}

// SAFETY: parameters are safe to invalidate after submit
unsafe impl Operation for MkDir<'_> {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::MkDirAt::new(self.location.parent_fd(), self.location.path_ptr())
            .mode(self.mode.bits())
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

/// Rename a file.
///
/// Corresponds to [`io_uring_prep_renameat(3)`](https://man7.org/linux/man-pages/man3/io_uring_prep_renameat.3.html).
#[must_use]
pub struct Rename<'parameters> {
    old: Location<'parameters>,
    new: Location<'parameters>,
    flags: RenameFlags,
}

impl<'parameters> Rename<'parameters> {
    pub const fn new(
        old: Location<'parameters>,
        new: Location<'parameters>,
        flags: RenameFlags,
    ) -> Self {
        Self { old, new, flags }
    }
}

// SAFETY: parameters are safe to invalidate after submit
unsafe impl Operation for Rename<'_> {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::RenameAt::new(
            self.old.parent_fd(),
            self.old.path_ptr(),
            self.new.parent_fd(),
            self.new.path_ptr(),
        )
        .flags(self.flags.bits())
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

/// Create a hard link.
///
/// Corresponds to [`io_uring_prep_linkat(3)`](https://man7.org/linux/man-pages/man3/io_uring_prep_linkat.3.html).
#[must_use]
pub struct Link<'parameters> {
    old: Location<'parameters>,
    new: Location<'parameters>,
    flags: LinkatFlags,
}

impl<'parameters> Link<'parameters> {
    pub const fn new(
        old: Location<'parameters>,
        new: Location<'parameters>,
        flags: LinkatFlags,
    ) -> Self {
        Self { old, new, flags }
    }
}

// SAFETY: parameters are safe to invalidate after submit
unsafe impl Operation for Link<'_> {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::LinkAt::new(
            self.old.parent_fd(),
            self.old.path_ptr(),
            self.new.parent_fd(),
            self.new.path_ptr(),
        )
        .flags(self.flags.bits())
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

/// Create a symbolic link.
///
/// Corresponds to [`io_uring_prep_symlinkat(3)`](https://man7.org/linux/man-pages/man3/io_uring_prep_symlinkat.3.html).
#[must_use]
pub struct Symlink<'parameters> {
    location: Location<'parameters>,
    target: &'parameters CStr,
}

impl<'parameters> Symlink<'parameters> {
    pub const fn new(location: Location<'parameters>, target: &'parameters CStr) -> Self {
        Self { location, target }
    }
}

// SAFETY: parameters are safe to invalidate after submit
unsafe impl Operation for Symlink<'_> {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::SymlinkAt::new(
            self.location.parent_fd(),
            self.target.as_ptr().cast(),
            self.location.path_ptr(),
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

/// Remove a filename and possibly the file itself.
///
/// Corresponds to [`io_uring_prep_unlinkat(3)`](https://man7.org/linux/man-pages/man3/io_uring_prep_unlinkat.3.html).
#[must_use]
pub struct Unlink<'parameters> {
    location: Location<'parameters>,
    flags: UnlinkatFlags,
}

impl<'parameters> Unlink<'parameters> {
    pub const fn new(location: Location<'parameters>, flags: UnlinkatFlags) -> Self {
        Self { location, flags }
    }
}

// SAFETY: parameters are safe to invalidate after submit
unsafe impl Operation for Unlink<'_> {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        let flags = match self.flags {
            UnlinkatFlags::RemoveDir => AtFlags::AT_REMOVEDIR,
            UnlinkatFlags::NoRemoveDir => AtFlags::empty(),
        };

        opcode::UnlinkAt::new(self.location.parent_fd(), self.location.path_ptr())
            .flags(flags.bits())
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

/// File integrity sync operation.
///
/// Corresponds to [`io_uring_prep_fsync(3)`](https://man7.org/linux/man-pages/man3/io_uring_prep_fsync.3.html).
#[must_use]
pub struct Fsync {
    file: UringFd,
}

impl Fsync {
    pub const fn new(file: UringFd) -> Self {
        Self { file }
    }
}

// SAFETY: no parameters to invalidate
unsafe impl Operation for Fsync {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        match self.file {
            UringFd::Fd(fd) => opcode::Fsync::new(Fd(fd)),
            UringFd::Fixed(index) => opcode::Fsync::new(Fixed(index)),
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
