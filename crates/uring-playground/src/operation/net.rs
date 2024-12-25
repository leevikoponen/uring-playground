use std::{
    any::Any,
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
use nix::sys::socket::{
    AddressFamily,
    Backlog,
    SockFlag,
    SockProtocol,
    SockType,
    SockaddrLike as _,
    SockaddrStorage,
};

use crate::{
    driver::{FileIndex, UringFd},
    operation::Operation,
};

/// Create a network socket.
///
/// Corresponds to [`io_uring_prep_socket(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_socket.3.html).
#[must_use]
pub struct Socket<T> {
    domain: AddressFamily,
    kind: SockType,
    protocol: SockProtocol,
    flags: SockFlag,
    marker: PhantomData<T>,
}

impl Socket<RawFd> {
    pub const fn new(
        domain: AddressFamily,
        kind: SockType,
        protocol: SockProtocol,
        flags: SockFlag,
    ) -> Self {
        Self {
            domain,
            kind,
            protocol,
            flags,
            marker: PhantomData,
        }
    }

    /// Utilize a fixed file descriptor instead.
    pub const fn use_fixed(self) -> Socket<FileIndex> {
        Socket {
            domain: self.domain,
            kind: self.kind,
            protocol: self.protocol,
            flags: self.flags,
            marker: PhantomData,
        }
    }
}

// SAFETY: nothing to be invalidated
unsafe impl Operation for Socket<RawFd> {
    type Output = Result<RawFd>;

    #[expect(clippy::as_conversions, reason = "unfortunate api design in nix")]
    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::Socket::new(self.domain as _, self.kind as _, self.protocol as _)
            .flags(self.flags.bits())
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

// SAFETY: nothing to be invalidated
unsafe impl Operation for Socket<FileIndex> {
    type Output = Result<FileIndex>;

    #[expect(clippy::as_conversions, reason = "unfortunate api design in nix")]
    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::Socket::new(self.domain as _, self.kind as _, self.protocol as _)
            .flags(self.flags.bits())
            .file_index(Some(DestinationSlot::auto_target()))
            .build()
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        if entry.result().is_negative() {
            return Err(Error::from_raw_os_error(-entry.result()));
        }

        let output = entry
            .result()
            .try_into()
            .expect("fixed file index should fit into it's appropriate type");

        Ok(output)
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        None
    }
}

/// Connect a socket to a remote address.
///
/// Corresponds to [`io_uring_prep_connect(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_connect.3.html).
#[must_use]
pub struct Connect {
    socket: UringFd,
    address: SockaddrStorage,
}

impl Connect {
    pub const fn new(socket: UringFd, address: SockaddrStorage) -> Self {
        Self { socket, address }
    }
}

// SAFETY: lifetimes bound correctly
unsafe impl Operation for Connect {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        match self.socket {
            UringFd::Fd(socket) => {
                opcode::Connect::new(Fd(socket), self.address.as_ptr(), self.address.len())
            }
            UringFd::Fixed(socket) => {
                opcode::Connect::new(Fixed(socket), self.address.as_ptr(), self.address.len())
            }
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

/// Bind a socket to an address.
///
/// Corresponds to [`io_uring_prep_bind(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_bind.3.html).
#[must_use]
pub struct Bind {
    socket: UringFd,
    address: SockaddrStorage,
}

impl Bind {
    pub const fn new(socket: UringFd, address: SockaddrStorage) -> Self {
        Self { socket, address }
    }
}

// SAFETY: lifetimes bound correctly
unsafe impl Operation for Bind {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        match self.socket {
            UringFd::Fd(socket) => {
                opcode::Bind::new(Fd(socket), self.address.as_ptr(), self.address.len())
            }
            UringFd::Fixed(socket) => {
                opcode::Bind::new(Fixed(socket), self.address.as_ptr(), self.address.len())
            }
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

/// Start listening for connections.
///
/// Corresponds to [`io_uring_prep_listen(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_listen.3.html).
#[must_use]
pub struct Listen {
    socket: UringFd,
    backlog: Backlog,
}

impl Listen {
    pub const fn new(socket: UringFd, backlog: Backlog) -> Self {
        Self { socket, backlog }
    }
}

// SAFETY: lifetimes bound correctly
unsafe impl Operation for Listen {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        match self.socket {
            UringFd::Fd(socket) => opcode::Listen::new(Fd(socket), self.backlog.into()),
            UringFd::Fixed(socket) => opcode::Listen::new(Fixed(socket), self.backlog.into()),
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

/// Shut the connection down.
///
/// Corresponds to [`io_uring_prep_shutdown(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_shutdown.3.html).
#[must_use]
pub struct Shutdown {
    socket: UringFd,
}

impl Shutdown {
    pub const fn new(socket: UringFd) -> Self {
        Self { socket }
    }
}

// SAFETY: lifetimes bound correctly
unsafe impl Operation for Shutdown {
    type Output = Result<()>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        match self.socket {
            UringFd::Fd(socket) => opcode::Shutdown::new(Fd(socket), libc::SHUT_RDWR),
            UringFd::Fixed(socket) => opcode::Shutdown::new(Fixed(socket), libc::SHUT_RDWR),
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

/// Socket address storage and length coupled together for easy heap allocation.
#[must_use]
pub struct AddressBuilder {
    storage: libc::sockaddr_storage,
    length: libc::socklen_t,
}

impl AddressBuilder {
    /// The full length of the storage part.
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        reason = "the length type is literally meant for this struct's size"
    )]
    const STORAGE_LENGTH: libc::socklen_t = size_of::<libc::sockaddr_storage>() as _;

    /// Initialize in an empty state.
    pub const fn empty() -> Self {
        Self {
            // SAFETY: zeroed value is considered valid
            storage: unsafe { std::mem::zeroed() },
            length: Self::STORAGE_LENGTH,
        }
    }

    /// Get pointers for the separate parts expected by OS interfaces.
    pub const fn as_raw_parts_ptr_mut(&mut self) -> (*mut libc::sockaddr, *mut libc::socklen_t) {
        let this = std::ptr::from_mut(self);
        // SAFETY: correctly slicing with field offsets
        let storage = unsafe { this.byte_add(std::mem::offset_of!(Self, storage)).cast() };
        // SAFETY: correctly slicing with field offsets
        let length = unsafe { this.byte_add(std::mem::offset_of!(Self, length)).cast() };

        (storage, length)
    }

    /// Assume that the pointers have been used to fill the value.
    ///
    /// # Safety
    ///
    /// The address and length must have been filled with valid values.
    ///
    /// # Panics
    ///
    /// If the address is detactably in an invalid state.
    #[must_use]
    pub unsafe fn finish_assuming_initialized(self) -> SockaddrStorage {
        // SAFETY: we are promised that the values were filled somehow
        unsafe {
            SockaddrStorage::from_raw(std::ptr::from_ref(&self.storage).cast(), Some(self.length))
        }
        .expect("filled address builder should be in a valid state")
    }
}

impl Default for AddressBuilder {
    fn default() -> Self {
        Self::empty()
    }
}

/// Accept a connection as a oneshot request.
///
/// Corresponds to [`io_uring_prep_accept(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_accept.3.html).
#[must_use]
pub struct Accept<T> {
    socket: T,
    flags: SockFlag,
    address: Option<Box<AddressBuilder>>,
}

impl<T> Accept<T> {
    pub const fn new(socket: T, flags: SockFlag) -> Self {
        Self {
            socket,
            flags,
            address: None,
        }
    }

    pub fn capturing_address(mut self, choice: bool) -> Self {
        self.address = choice.then(Box::default);
        self
    }
}

// SAFETY: the address is passed on correctly
unsafe impl Operation for Accept<RawFd> {
    type Output = Result<(RawFd, Option<SockaddrStorage>)>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        let this = self.get_mut();
        let (storage, length) = this.address.as_deref_mut().map_or(
            (std::ptr::null_mut(), std::ptr::null_mut()),
            AddressBuilder::as_raw_parts_ptr_mut,
        );

        opcode::Accept::new(Fd(this.socket), storage, length)
            .flags(this.flags.bits())
            .build()
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        if entry.result().is_negative() {
            return Err(Error::from_raw_os_error(-entry.result()));
        }

        Ok((
            entry.result(),
            self.get_mut().address.take().map(|builder| {
                // SAFETY: the kernel should have filled the storage and length correctly
                unsafe { builder.finish_assuming_initialized() }
            }),
        ))
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        let allocation = self.get_mut().address.take()?;

        Some(Box::new(allocation))
    }
}

// SAFETY: the address is passed on correctly
unsafe impl Operation for Accept<FileIndex> {
    type Output = Result<(FileIndex, Option<SockaddrStorage>)>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        let this = self.get_mut();
        let (storage, length) = this.address.as_deref_mut().map_or(
            (std::ptr::null_mut(), std::ptr::null_mut()),
            AddressBuilder::as_raw_parts_ptr_mut,
        );

        opcode::Accept::new(Fixed(this.socket), storage, length)
            .flags(this.flags.bits())
            .build()
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        let socket = entry
            .result()
            .try_into()
            .map_err(|_| Error::from_raw_os_error(-entry.result()))?;

        let address = self.get_mut().address.take().map(|builder| {
            // SAFETY: the kernel should have filled the storage and length correctly
            unsafe { builder.finish_assuming_initialized() }
        });

        Ok((socket, address))
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        let allocation = self.get_mut().address.take()?;

        Some(Box::new(allocation))
    }
}

/// Accept connections as a multishot request.
///
/// Corresponds to [`io_uring_prep_multishot_accept(3)`](https://www.man7.org/linux/man-pages/man3/io_uring_prep_multishot_accept.3.html).
#[must_use]
pub struct AcceptMulti<T> {
    socket: T,
    flags: SockFlag,
}

impl<T> AcceptMulti<T> {
    pub const fn new(socket: T, flags: SockFlag) -> Self {
        Self { socket, flags }
    }
}

// SAFETY: nothing to invalidate
unsafe impl Operation for AcceptMulti<RawFd> {
    type Output = Result<RawFd>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::AcceptMulti::new(Fd(self.socket))
            .flags(self.flags.bits())
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

// SAFETY: nothing to invalidate
unsafe impl Operation for AcceptMulti<FileIndex> {
    type Output = Result<FileIndex>;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        opcode::AcceptMulti::new(Fixed(self.socket))
            .flags(self.flags.bits())
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
