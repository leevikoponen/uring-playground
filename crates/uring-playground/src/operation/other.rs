use std::{
    any::Any,
    io::{Error, Result},
    os::fd::RawFd,
    pin::Pin,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use io_uring::{
    cqueue,
    opcode,
    squeue,
    types::{DestinationSlot, Fd, Fixed, TimeoutFlags, Timespec},
};

use crate::{
    driver::{OperationId, FileIndex},
    operation::Operation,
};

/// Unchecked operation that promises to be memory safe.
#[must_use]
pub struct Raw {
    inner: Option<squeue::Entry>,
}

impl Raw {
    /// Create an operation with a raw submission queue entry.
    ///
    /// # Safety
    ///
    /// The rules as described in [`Operation`] must be upheld.
    pub const unsafe fn new(entry: squeue::Entry) -> Self {
        Self { inner: Some(entry) }
    }
}

// SAFETY: unsafe to construct
unsafe impl Operation for Raw {
    type Output = cqueue::Entry;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        self.get_mut()
            .inner
            .take()
            .expect("operations should only be used once")
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        entry
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        None
    }
}

/// Operation that does nothing.
///
/// Corresponds to [`io_uring_prep_nop(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_nop.3.html).
// TODO: support fault injection
#[must_use]
#[non_exhaustive]
pub struct Nop;

impl Nop {
    #[expect(
        clippy::new_without_default,
        reason = "default for operations doesn't feel right"
    )]
    pub const fn new() -> Self {
        Self {}
    }
}

// SAFETY: no parameters to invalidate
unsafe impl Operation for Nop {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::Nop::new().build()
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

/// Timeout for a set of linked operations.
///
/// Corresponds to [`io_uring_prep_link_timeout(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_link_timeout.3.html).
#[must_use]
pub struct LinkedTimeout {
    value: Timespec,
    flags: TimeoutFlags,
}

impl LinkedTimeout {
    /// Use a relative timeout.
    ///
    /// # Notes
    ///
    /// Do notice that the duration is tied to when the operation is finally
    /// submitted to the kernel, not to this initial instantiation. This
    /// shouldn't be a huge difference in well behaved applications unless
    /// particularly high accuracy is required.
    pub fn relative(value: Duration) -> Self {
        Self {
            value: Timespec::from(value),
            flags: TimeoutFlags::empty(),
        }
    }

    /// Use an absolute timeout.
    ///
    /// # Notes
    ///
    /// It really is unfortunate that we can't get at the internal value of
    /// [`std::time::Instant`] and must use the realtime clock instead of the
    /// kernel's monotonic clock, making this function somewhat less useful.
    pub fn absolute(value: SystemTime) -> Self {
        let value = value
            .duration_since(UNIX_EPOCH)
            .map(Timespec::from)
            .unwrap_or_default();

        Self {
            value,
            flags: TimeoutFlags::ABS | TimeoutFlags::REALTIME,
        }
    }
}

impl From<Duration> for LinkedTimeout {
    fn from(value: Duration) -> Self {
        Self::relative(value)
    }
}

impl From<SystemTime> for LinkedTimeout {
    fn from(value: SystemTime) -> Self {
        Self::absolute(value)
    }
}

// SAFETY: parameters are safe to invalidate after submit
unsafe impl Operation for LinkedTimeout {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::LinkTimeout::new(&raw const self.value)
            .flags(self.flags | TimeoutFlags::ETIME_SUCCESS)
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

/// Send a fixed file descriptor to another ring instance.
///
/// Corresponds to  [`io_uring_prep_msg_ring_fd(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_msg_ring_fd.3.html).
#[must_use]
pub struct MsgRingSendFd {
    receiver: RawFd,
    waiter: OperationId,
    file: FileIndex,
}

impl MsgRingSendFd {
    pub const fn new(receiver: RawFd, waiter: OperationId, file: FileIndex) -> Self {
        Self {
            receiver,
            waiter,
            file,
        }
    }
}

// SAFETY: no parameters to invalidate
unsafe impl Operation for MsgRingSendFd {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::MsgRingSendFd::new(
            Fd(self.receiver),
            Fixed(self.file),
            DestinationSlot::auto_target(),
            self.waiter.user_data_bits(),
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
