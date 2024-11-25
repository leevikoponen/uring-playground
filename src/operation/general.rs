use std::{
    any::Any,
    io::{Error, Result},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use io_uring::{
    cqueue,
    opcode,
    squeue,
    types::{TimeoutFlags, Timespec},
};

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

/// Operation that connects a timeout to linked operations.
///
/// Corresponds to [io_uring_prep_link_timeout(3)](https://www.man7.org/linux/man-pages/man3/io_uring_prep_link_timeout.3.html).
#[must_use]
pub struct LinkTimeout {
    time: Timespec,
    flags: TimeoutFlags,
}

impl LinkTimeout {
    pub fn relative(duration: Duration) -> Self {
        Self {
            time: Timespec::from(duration),
            flags: TimeoutFlags::empty(),
        }
    }

    pub fn absolute(time: SystemTime) -> Self {
        Self {
            time: time
                .duration_since(UNIX_EPOCH)
                .map(Timespec::from)
                .unwrap_or_default(),
            flags: TimeoutFlags::ABS | TimeoutFlags::REALTIME,
        }
    }
}

// SAFETY: no parameters to invalidate
unsafe impl Operation for LinkTimeout {
    type Output = Result<()>;

    fn build_submission(&mut self) -> squeue::Entry {
        opcode::LinkTimeout::new(&self.time)
            .flags(self.flags)
            .build()
    }

    unsafe fn handle_completion(&mut self, entry: cqueue::Entry) -> Self::Output {
        if entry.result().is_negative() {
            return Err(Error::from_raw_os_error(-entry.result()));
        }

        Ok(())
    }

    fn take_required_allocations(&mut self) -> Option<Box<dyn Any>> {
        None
    }
}

// SAFETY: only returns once
unsafe impl Oneshot for LinkTimeout {}
