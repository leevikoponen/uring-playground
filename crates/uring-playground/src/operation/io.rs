use std::{
    any::Any,
    io::{Error, Result},
    pin::Pin,
};

use io_uring::{
    cqueue,
    opcode,
    squeue,
    types::{Fd, Fixed},
};

use crate::{
    driver::{BufferEntry, BufferRegistration, Endpoint, StableBuffer, UringFd},
    operation::Operation,
};

/// Read from a file.
///
/// Corresponds to [`io_uring_prep_read(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_read.3.html).
#[must_use]
pub struct Read<B> {
    endpoint: Endpoint,
    buffer: B,
    limit: Option<usize>,
}

impl<B> Read<B> {
    /// Calculate the length parameter.
    fn calculate_len(&self, available: usize) -> u32 {
        self.limit
            .unwrap_or(usize::MAX)
            .min(available)
            .try_into()
            .unwrap_or(u32::MAX)
    }

    pub const fn new(endpoint: Endpoint, buffer: B) -> Self {
        Self {
            endpoint,
            buffer,
            limit: None,
        }
    }

    /// Limit the maximum amount of data to be read.
    pub const fn with_limit(mut self, limit: Option<usize>) -> Self {
        self.limit = limit;
        self
    }
}

impl<'parameters> Read<&'parameters BufferRegistration<'parameters>> {
    /// Utilize a multishot operation instead.
    pub const fn use_multishot(self) -> ReadMulti<'parameters> {
        ReadMulti { inner: self }
    }
}

// SAFETY: the mutable buffer is yielded out as required
unsafe impl<B: StableBuffer> Operation for Read<B> {
    type Output = std::result::Result<B, (Error, B)>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        let this = self.get_mut();
        let (pointer, remaining) = this.buffer.unfilled_section();

        match this.endpoint.file {
            UringFd::Fd(file) => {
                opcode::Read::new(Fd(file), pointer, this.calculate_len(remaining))
            }
            UringFd::Fixed(file) => {
                opcode::Read::new(Fixed(file), pointer, this.calculate_len(remaining))
            }
        }
        .offset(this.endpoint.offset.into_rw_repr())
        .build()
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        let this = self.get_mut();
        if entry.result().is_negative() {
            // SAFETY: the operation is complete and not initializing anything
            return Err((Error::from_raw_os_error(-entry.result()), unsafe {
                this.buffer.take_ownership(0)
            }));
        }

        // SAFETY: we have to trust the kernel
        Ok(unsafe {
            this.buffer
                .take_ownership(entry.result().try_into().unwrap_or(usize::MAX))
        })
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        let this = self.get_mut();

        // SAFETY: not marking any extra data as written
        Some(Box::new(unsafe { this.buffer.take_ownership(0) }))
    }
}

// SAFETY: parameters are bound to live long enough
unsafe impl<'parameters> Operation for Read<&'parameters BufferRegistration<'parameters>> {
    type Output = Result<BufferEntry<'parameters>>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        match self.endpoint.file {
            UringFd::Fd(file) => opcode::Read::new(
                Fd(file),
                std::ptr::null_mut(),
                self.calculate_len(usize::MAX),
            ),
            UringFd::Fixed(file) => opcode::Read::new(
                Fixed(file),
                std::ptr::null_mut(),
                self.calculate_len(usize::MAX),
            ),
        }
        .offset(self.endpoint.offset.into_rw_repr())
        .buf_group(self.buffer.group_id())
        .build()
        .flags(squeue::Flags::BUFFER_SELECT)
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        let amount = entry
            .result()
            .try_into()
            .map_err(|_| Error::from_raw_os_error(-entry.result()))?;

        let inner = io_uring::cqueue::buffer_select(entry.flags())
            .expect("multishot read should return buffer id");

        // SAFETY: completion is guaranteed to correspond to our submission
        Ok(unsafe { self.buffer.resolve_buffer(inner, amount) })
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        None
    }
}

/// Read from a file as a multishot stream.
///
/// Corresponds to [`io_uring_prep_read_multishot(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_read_multishot.3.html).
#[must_use]
pub struct ReadMulti<'parameters> {
    inner: Read<&'parameters BufferRegistration<'parameters>>,
}

// SAFETY: parameters are bound to live long enough
unsafe impl<'parameters> Operation for ReadMulti<'parameters> {
    type Output = Result<BufferEntry<'parameters>>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        match self.inner.endpoint.file {
            UringFd::Fd(file) => opcode::ReadMulti::new(
                Fd(file),
                self.inner.calculate_len(usize::MAX),
                self.inner.buffer.group_id(),
            ),
            UringFd::Fixed(file) => opcode::ReadMulti::new(
                Fixed(file),
                self.inner.calculate_len(usize::MAX),
                self.inner.buffer.group_id(),
            ),
        }
        .offset(self.inner.endpoint.offset.into_rw_repr())
        .build()
        .flags(squeue::Flags::BUFFER_SELECT)
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        let amount = entry
            .result()
            .try_into()
            .map_err(|_| Error::from_raw_os_error(-entry.result()))?;

        let inner = io_uring::cqueue::buffer_select(entry.flags())
            .expect("multishot read should return buffer id");

        // SAFETY: completion is guaranteed to correspond to our submission
        Ok(unsafe { self.inner.buffer.resolve_buffer(inner, amount) })
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        None
    }
}

/// Write to a file.
///
/// Corresponds to [io_uring_prep_write(3)](https://www.man7.org/linux/man-pages/man3/io_uring_prep_write.3.html).
#[must_use]
pub struct Write<'parameters> {
    endpoint: Endpoint,
    buffer: &'parameters [u8],
}

impl<'parameters> Write<'parameters> {
    pub const fn new(endpoint: Endpoint, buffer: &'parameters [u8]) -> Self {
        Self { endpoint, buffer }
    }
}

// SAFETY: parameters are safe to invalidate after submit
unsafe impl Operation for Write<'_> {
    type Output = Result<usize>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        match self.endpoint.file {
            UringFd::Fd(file) => opcode::Write::new(
                Fd(file),
                self.buffer.as_ptr(),
                self.buffer.len().try_into().unwrap_or(u32::MAX),
            ),
            UringFd::Fixed(file) => opcode::Write::new(
                Fixed(file),
                self.buffer.as_ptr(),
                self.buffer.len().try_into().unwrap_or(u32::MAX),
            ),
        }
        .offset(self.endpoint.offset.into_rw_repr())
        .build()
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        if entry.result().is_negative() {
            return Err(Error::from_raw_os_error(-entry.result()));
        }

        Ok(entry.result().try_into().unwrap_or(usize::MAX))
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        None
    }
}

/// Move data between file descriptors.
///
/// Corresponds to [`io_uring_prep_splice(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_splice.3.html).
#[must_use]
pub struct Splice {
    input: Endpoint,
    output: Endpoint,
    length: usize,
}

impl Splice {
    pub const fn new(input: Endpoint, output: Endpoint, length: usize) -> Self {
        Self {
            input,
            output,
            length,
        }
    }
}

// SAFETY: lifetimes are bound correctly
unsafe impl Operation for Splice {
    type Output = Result<usize>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        match (self.input.file, self.output.file) {
            (UringFd::Fd(input), UringFd::Fd(output)) => opcode::Splice::new(
                Fd(input),
                self.input.offset.into_splice_repr(),
                Fd(output),
                self.output.offset.into_splice_repr(),
                self.length.try_into().unwrap_or(u32::MAX),
            ),
            (UringFd::Fd(input), UringFd::Fixed(output)) => opcode::Splice::new(
                Fd(input),
                self.input.offset.into_splice_repr(),
                Fixed(output),
                self.output.offset.into_splice_repr(),
                self.length.try_into().unwrap_or(u32::MAX),
            ),
            (UringFd::Fixed(input), UringFd::Fd(output)) => opcode::Splice::new(
                Fixed(input),
                self.input.offset.into_splice_repr(),
                Fd(output),
                self.output.offset.into_splice_repr(),
                self.length.try_into().unwrap_or(u32::MAX),
            ),
            (UringFd::Fixed(input), UringFd::Fixed(output)) => opcode::Splice::new(
                Fixed(input),
                self.input.offset.into_splice_repr(),
                Fixed(output),
                self.output.offset.into_splice_repr(),
                self.length.try_into().unwrap_or(u32::MAX),
            ),
        }
        .build()
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        if entry.result().is_negative() {
            return Err(Error::from_raw_os_error(-entry.result()));
        }

        Ok(entry.result().try_into().unwrap_or(usize::MAX))
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        None
    }
}
