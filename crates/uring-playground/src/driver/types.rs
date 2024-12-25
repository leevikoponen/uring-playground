use std::{ffi::CStr, os::fd::RawFd};

use io_uring::types::Fd;

/// Raw fixed file descriptor index.
pub type FileIndex = u32;

/// Either type of raw file descriptor.
#[derive(Clone, Copy)]
pub enum UringFd {
    Fd(RawFd),
    Fixed(FileIndex),
}

impl From<RawFd> for UringFd {
    fn from(value: RawFd) -> Self {
        Self::Fd(value)
    }
}

impl From<FileIndex> for UringFd {
    fn from(value: FileIndex) -> Self {
        Self::Fixed(value)
    }
}

/// Some kind of buffer that can conceptually give away the ownership of an
/// uninitialized part to the kernel.
///
/// # Safety
///
/// The section yielded has to be effectively pinned and safe to be treated as
/// owned by the kernel for the duration of an operation.
///
/// It is promised that the buffer is always either leaked or the ownership
/// stashed somewhere without marking anything as written and is not touched
/// before the operation completes.
#[must_use]
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not marked as a `StableBuffer`",
    note = "`io_uring` requires transferring ownership to kernel during operations"
)]
pub unsafe trait StableBuffer: Send + Sized + Unpin + 'static {
    /// Get the raw pointer and length to unfilled capacity.
    fn unfilled_section(&mut self) -> (*mut u8, usize);

    /// Extract the buffer, consuming back ownership of the newly filled part.
    ///
    /// # Safety
    ///
    /// At least the given amount has to have been written to the buffer.
    #[must_use]
    unsafe fn take_ownership(&mut self, written: usize) -> Self;
}

// SAFETY: the vector's allocation is effectively pinned correctly
unsafe impl StableBuffer for Vec<u8> {
    fn unfilled_section(&mut self) -> (*mut u8, usize) {
        // SAFETY: correctly slicing into the start of the unused capacity
        let start = unsafe { self.as_mut_ptr().add(self.len()) };
        let length = self.capacity() - self.len();

        (start, length)
    }

    unsafe fn take_ownership(&mut self, written: usize) -> Self {
        // SAFETY: caller guarantees that the section has been filled
        unsafe {
            self.set_len(self.len() + written);
        }

        std::mem::take(self)
    }
}

/// The behaviour of dealing with the file offset on the kernel side.
#[derive(Default, Clone, Copy)]
#[must_use]
pub enum SeekStrategy {
    #[default]
    Automatic,
    Disabled,
    Offset(usize),
}

impl SeekStrategy {
    /// Get a value corresponding to what read and write operations expect.
    ///
    /// # Panics
    ///
    /// If the value doesn't make sense.
    #[must_use]
    pub fn into_rw_repr(self) -> u64 {
        match self {
            Self::Automatic => u64::MAX,
            Self::Disabled => u64::MIN,
            Self::Offset(value) => value
                .try_into()
                .expect("seek offset shouldn't reasonably overflow"),
        }
    }

    /// Get a value corresponding to what the splice operation expects.
    ///
    /// # Panics
    ///
    /// If the value doesn't make sense.
    #[must_use]
    pub fn into_splice_repr(self) -> i64 {
        match self {
            Self::Automatic | Self::Disabled => -1,
            Self::Offset(value) => value
                .try_into()
                .expect("seek offset shouldn't reasonably overflow"),
        }
    }
}

/// Something representing a source or destination for I/O with configurable
/// seek behaviour.
#[must_use]
pub struct Endpoint {
    pub(crate) file: UringFd,
    pub(crate) offset: SeekStrategy,
}

impl Endpoint {
    /// Build an endpoint using the given file descriptor.
    pub const fn new(file: UringFd) -> Self {
        Self {
            file,
            offset: SeekStrategy::Automatic,
        }
    }

    /// Set whether the file offset should be moved with operations.
    pub const fn with_seek(mut self, enabled: bool) -> Self {
        self.offset = if enabled {
            SeekStrategy::Automatic
        } else {
            SeekStrategy::Disabled
        };
        self
    }

    /// Use a specific absolute file offset with operations.
    pub const fn with_offset(mut self, offset: usize) -> Self {
        self.offset = SeekStrategy::Offset(offset);
        self
    }
}

/// Potentially scoped path on the file system.
#[must_use]
pub struct Location<'parameters> {
    parent: Option<RawFd>,
    path: &'parameters CStr,
}

impl<'parameters> Location<'parameters> {
    /// Get the path pointer parameter for operations.
    pub(crate) const fn path_ptr(&self) -> *const libc::c_char {
        self.path.as_ptr().cast()
    }

    /// Build the directory file descriptor parameter for operations.
    pub(crate) fn parent_fd(&self) -> Fd {
        Fd(self.parent.unwrap_or(libc::AT_FDCWD))
    }

    /// Build a location with the given path.
    pub const fn new(path: &'parameters CStr) -> Self {
        Self { parent: None, path }
    }

    /// Set where the path is resolved from.
    pub const fn with_parent(mut self, parent: Option<RawFd>) -> Self {
        self.parent = parent;
        self
    }
}
