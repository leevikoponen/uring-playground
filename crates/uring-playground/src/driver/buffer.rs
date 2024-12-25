use std::{
    cell::Cell,
    io::Result,
    ops::Deref,
    sync::atomic::{AtomicU16, Ordering},
};

use io_uring::types::BufRingEntry;
use memmap2::{Advice, MmapMut};

use crate::driver::Reactor;

/// Internal storage of ring mapped buffers.
///
/// # Notes
///
/// Effectively backed by a slightly weirdly implemented ring buffer shared with
/// the kernel that contains buffer metadata and another allocation for the
/// buffers themselves.
#[must_use]
pub struct BufferRing {
    buffer_storage: MmapMut,
    ring_storage: MmapMut,

    buffer_size: usize,
    entry_count: u16,

    actual_tail: *const AtomicU16,
    cached_tail: Cell<u16>,
}

impl BufferRing {
    /// Allocate a new buffer ring.
    ///
    /// # Errors
    ///
    /// If creating the required memory mappings fails.
    pub fn allocate(entry_count: u16, buffer_size: usize) -> Result<Self> {
        let buffer_storage = MmapMut::map_anon(usize::from(entry_count) * buffer_size)?;
        let ring_storage = MmapMut::map_anon(usize::from(entry_count) * size_of::<BufRingEntry>())?;

        for mapping in [&buffer_storage, &ring_storage] {
            for request in [Advice::DontFork, Advice::DontDump] {
                mapping.advise(request)?;
            }
        }

        Ok(Self {
            // SAFETY: we have allocated the ring according to the requirements
            actual_tail: unsafe { BufRingEntry::tail(ring_storage.as_ptr().cast()).cast() },
            cached_tail: Cell::new(0),

            ring_storage,
            buffer_storage,

            buffer_size,
            entry_count,
        })
    }

    /// Get a pointer to a buffer based on it's identifier.
    ///
    /// # Safety
    ///
    /// The identifier must be within bounds of the entries in this ring.
    ///
    /// # Notes
    ///
    /// When dereferencing you must ensure that this identifier originated from
    /// the kernel conceptually giving ownership during an completion.
    unsafe fn resolve_buffer_pointer(&self, id: u16) -> *const u8 {
        // SAFETY: correctly calculating offset according to pool properties
        unsafe {
            self.buffer_storage
                .as_ptr()
                .add(usize::from(id) * self.buffer_size)
                .cast()
        }
    }

    /// Push the specified entry to the tail of the buffer ring.
    ///
    /// # Safety
    ///
    /// The buffer's ownership given back and it should not be touched while
    /// conceptually owned by the kernel.
    #[expect(
        clippy::as_conversions,
        clippy::cast_ptr_alignment,
        reason = "funky casts unfortunately required by how we do things"
    )]
    unsafe fn push_buffer_entry(&self, id: u16) {
        let previous_tail = self.cached_tail.get();
        self.cached_tail.set(previous_tail.wrapping_add(1));

        // SAFETY: correctly calculating ring offset
        let entry_pointer = unsafe {
            self.ring_storage
                .as_ptr()
                .cast_mut()
                .cast::<BufRingEntry>()
                .add(usize::from(previous_tail & (self.entry_count - 1)))
        };

        // SAFETY: the entry should have been indexed into correctly
        let entry = unsafe { &mut *entry_pointer };

        // SAFETY: using the pointer to give it back to the kernel
        entry.set_addr(unsafe { self.resolve_buffer_pointer(id) } as _);
        entry.set_len(self.buffer_size.try_into().unwrap_or(u32::MAX));
        entry.set_bid(id);
    }

    /// Make the current cached tail index visible to the kernel.
    fn synchronize_shared_tail(&self) {
        // SAFETY: the pointer should have originally been built correctly
        unsafe {
            (*self.actual_tail).store(self.cached_tail.get(), Ordering::Release);
        }
    }

    /// Register the buffer ring and make the buffer entries usable.
    ///
    /// # Errors
    ///
    /// If the registration fails.
    ///
    /// # Panics
    ///
    /// If the reactor runs out of possible buffer ID's.
    #[expect(clippy::as_conversions, reason = "unfortunate type definition quirks")]
    pub fn register_with_kernel<'context>(
        &'context self,
        reactor: &'context Reactor,
    ) -> Result<BufferRegistration<'context>> {
        let id = reactor.buffer_group_id_alloc.replace(
            reactor
                .buffer_group_id_alloc
                .get()
                .checked_add(1)
                .expect("shouldn't be reasonable to run out of buffer ring ids"),
        );

        // SAFETY: the ring has been allocated correctly
        unsafe {
            reactor.use_inner(|submitter| {
                submitter.register_buf_ring_with_flags(
                    self.actual_tail as _,
                    self.entry_count,
                    id,
                    0,
                )
            })?;
        }

        for index in 0..self.entry_count {
            // SAFETY: we're not keeping access to the section
            unsafe {
                self.push_buffer_entry(index);
            }
        }

        self.synchronize_shared_tail();

        Ok(BufferRegistration {
            reactor,
            ring: self,
            id,
        })
    }
}

/// Registered buffer pool.
#[must_use]
pub struct BufferRegistration<'context> {
    reactor: &'context Reactor,
    ring: &'context BufferRing,
    id: u16,
}

impl BufferRegistration<'_> {
    /// Get the group identifier for this pool.
    #[must_use]
    pub const fn group_id(&self) -> u16 {
        self.id
    }

    /// Get a buffer handle from this pool.
    ///
    /// # Safety
    ///
    /// The identifier and amount written must originate from an operation
    /// completion that used this pool.
    pub const unsafe fn resolve_buffer(&self, id: u16, length: usize) -> BufferEntry<'_> {
        BufferEntry {
            pool: self,
            id,
            length,
        }
    }
}

impl Drop for BufferRegistration<'_> {
    fn drop(&mut self) {
        self.reactor
            .use_inner(|submitter| submitter.unregister_buf_ring(self.id))
            .expect("should be able to unregister buffer ring on drop");
    }
}

/// Handle around a buffer entry in a ring mapped pool.
#[must_use]
pub struct BufferEntry<'parent> {
    pool: &'parent BufferRegistration<'parent>,
    id: u16,
    length: usize,
}

impl Deref for BufferEntry<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        // SAFETY: constructed with valid buffer identifier in the correct pool
        let start = unsafe { self.pool.ring.resolve_buffer_pointer(self.id) };
        // SAFETY: extracted from a completion with the indicated length
        unsafe { std::slice::from_raw_parts(start, self.length) }
    }
}

impl Drop for BufferEntry<'_> {
    fn drop(&mut self) {
        // SAFETY: the buffer accessor is being dropped here
        unsafe {
            self.pool.ring.push_buffer_entry(self.id);
        }

        self.pool.ring.synchronize_shared_tail();
    }
}

#[cfg(test)]
mod test {
    use std::{fs::File, os::fd::AsRawFd as _};

    use futures_lite::future;

    use crate::{
        driver::{BufferRing, Endpoint, Reactor, UringFd},
        operation::{Operation as _, Read},
    };

    #[test]
    fn can_use_buffer_pool() {
        let file = File::open("/dev/urandom").expect("should be able to open random source");
        let reactor = Reactor::initialize(16, None).expect("should be able to initialize reactor");
        let pool =
            BufferRing::allocate(16, 512).expect("should be able to allocate buffer storage");

        let pool = pool
            .register_with_kernel(&reactor)
            .expect("should be able to register buffer ring");

        future::block_on(future::or(
            async {
                let data = Read::new(
                    Endpoint::new(UringFd::Fd(file.as_raw_fd())).with_seek(false),
                    &pool,
                )
                .build_oneshot(&reactor)
                .await
                .expect("should be able to read from random source");

                assert!(
                    !data.is_empty(),
                    "should have read something into the buffer"
                );
            },
            async {
                reactor
                    .wait_for_progress(1, None, None)
                    .expect("should be able to wait for progress");

                future::yield_now().await;
                unreachable!("test should only be submitting and waiting once");
            },
        ));
    }
}
