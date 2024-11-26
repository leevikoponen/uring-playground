//! Compatibility wrapper for doing readiness based IO.
use std::{
    cell::RefCell,
    io::{Error, ErrorKind, Read, Result, Write},
    os::fd::AsRawFd,
    rc::Rc,
    task::{Context, Poll},
};

use io_uring::{
    opcode::{PollAdd, PollRemove, Shutdown},
    types::Fd,
};

use crate::reactor::{OperationId, Reactor};

// submodule for poll flags to allow the cast from libc's weird type choice
#[expect(clippy::as_conversions)]
mod poll {
    pub const READABLE: u32 = libc::POLLIN as u32;
    pub const WRITABLE: u32 = libc::POLLOUT as u32;
}

/// Adapter for doing IO operations through polling for readiness with
/// `io_uring` to allow for interfacing with most other libraries.
// TODO: consider using safer operation wrappers
#[must_use]
pub struct ReadinessBacked<T> {
    reactor: Rc<RefCell<Reactor>>,
    inner: T,
    read: Option<OperationId>,
    write: Option<OperationId>,
    shutdown: Option<OperationId>,
}

impl<T> ReadinessBacked<T> {
    pub const fn new(reactor: Rc<RefCell<Reactor>>, inner: T) -> Self {
        Self {
            reactor,
            inner,
            read: None,
            write: None,
            shutdown: None,
        }
    }
}

impl<T: AsRawFd + Read> ReadinessBacked<T> {
    /// Poll for data to be read into the specified buffer.
    pub fn poll_read(&mut self, context: &mut Context, buffer: &mut [u8]) -> Poll<Result<usize>> {
        loop {
            let operation = *self.read.get_or_insert_with(|| {
                // SAFETY: nothing to invalidate
                unsafe {
                    self.reactor.borrow_mut().queue_submission(
                        PollAdd::new(Fd(self.inner.as_raw_fd()), poll::READABLE).build(),
                        Some(context),
                    )
                }
            });

            let output = self
                .reactor
                .borrow_mut()
                .poll_completion(operation, context);

            let entry = std::task::ready!(output);
            self.read = None;

            if entry.result().is_negative() {
                return Poll::Ready(Err(Error::from_raw_os_error(-entry.result())));
            }

            match self.inner.read(buffer) {
                Ok(amount) => return Poll::Ready(Ok(amount)),
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == ErrorKind::WouldBlock => (),
                Err(error) => return Poll::Ready(Err(error)),
            }
        }
    }
}

impl<T: AsRawFd + Write> ReadinessBacked<T> {
    /// Poll for data to be written from the specified buffer.
    pub fn poll_write(&mut self, context: &mut Context, buffer: &[u8]) -> Poll<Result<usize>> {
        loop {
            let operation = *self.write.get_or_insert_with(|| {
                // SAFETY: nothing to invalidate
                unsafe {
                    self.reactor.borrow_mut().queue_submission(
                        PollAdd::new(Fd(self.inner.as_raw_fd()), poll::WRITABLE).build(),
                        Some(context),
                    )
                }
            });

            let output = self
                .reactor
                .borrow_mut()
                .poll_completion(operation, context);

            let entry = std::task::ready!(output);
            self.write = None;

            if entry.result().is_negative() {
                return Poll::Ready(Err(Error::from_raw_os_error(-entry.result())));
            }

            match self.inner.write(buffer) {
                Ok(amount) => return Poll::Ready(Ok(amount)),
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == ErrorKind::WouldBlock => (),
                Err(error) => return Poll::Ready(Err(error)),
            }
        }
    }
}

impl<T: AsRawFd> ReadinessBacked<T> {
    /// Poll for a socket to be gracefully shut down.
    pub fn poll_shutdown(&mut self, context: &mut Context) -> Poll<Result<()>> {
        loop {
            // TODO: use linked operations to remove all at once
            let mut dummy = None;
            let cancellable = if self.read.is_some() {
                &mut self.read
            } else if self.write.is_some() {
                &mut self.write
            } else {
                &mut dummy
            };

            match (cancellable, self.shutdown) {
                // ongoing poll request removal needs polling
                (slot @ Some(_), Some(id)) => {
                    let output = self.reactor.borrow_mut().poll_completion(id, context);
                    let entry = std::task::ready!(output);
                    self.shutdown = None;
                    *slot = None;

                    if entry.result().is_negative() {
                        return Poll::Ready(Err(Error::from_raw_os_error(-entry.result())));
                    }
                }
                // remaining poll request needs removal
                (Some(id), None) => {
                    // SAFETY: drop implementation forgets data
                    let operation = unsafe {
                        self.reactor.borrow_mut().queue_submission(
                            PollRemove::new(id.0.to_bits()).build(),
                            Some(context),
                        )
                    };

                    self.shutdown = Some(operation);
                }
                // ongoing shutdown needs polling
                (None, Some(id)) => {
                    let output = self.reactor.borrow_mut().poll_completion(id, context);
                    let entry = std::task::ready!(output);
                    self.shutdown = None;

                    if entry.result().is_negative() {
                        return Poll::Ready(Err(Error::from_raw_os_error(-entry.result())));
                    }

                    return Poll::Ready(Ok(()));
                }
                // shutdown needs to be started
                (None, None) => {
                    // SAFETY: drop implementation forgets data
                    let operation = unsafe {
                        self.reactor.borrow_mut().queue_submission(
                            Shutdown::new(Fd(self.inner.as_raw_fd()), libc::SHUT_RDWR).build(),
                            Some(context),
                        )
                    };

                    self.shutdown = Some(operation);
                }
            }
        }
    }
}

impl<T> Drop for ReadinessBacked<T> {
    fn drop(&mut self) {
        let mut reactor = self.reactor.borrow_mut();
        let operations = [self.read.take(), self.write.take(), self.shutdown.take()]
            .into_iter()
            .flatten();

        for id in operations {
            reactor.ignore_operation(id, None);
        }
    }
}
