//! Naive synchronization primitives built on top of futex operations.
//!
//! # Limitations and rationale
//!
//! It's worth noting that these sorts of textbook implementations are usually
//! not the most efficient for heavy concurrency, using `io_uring` for
//! synchronization in general is unlikely to somehow be magically fast even
//! though we're avoiding syscall overhead, since implementations focused on
//! high throughput and/or lower latency will already be avoiding syscalls under
//! load by being quite aggressive with spinning instead of quickly yielding.
//!
//! However, it can still be nice to utilize things like relatively high
//! `min_timeout` values to avoid unnecessary context switches while
//! communicating between components in the sort of lightly loaded actor model
//! like applications that I'm designing around.
//!
//! # Operation error handling
//!
//! I have essentially chosen not to implement any form of error handling in
//! these types, I can't realistically think what a synchronization primitive
//! could do in a case where a futex operation fails internally.
//!
//! As a result I've similarly chosen to not allow the use of timeouts or
//! graceful cancellation when it comes to these operations. It's absolutely not
//! trivial to figure out how to avoid ending up in an unexpected state when
//! some internal flag or counter value has been changed but the actual waiting
//! is no longer happening.
mod channel;
mod primitive;

pub use self::{
    channel::{ConcurrentQueue, ConcurrentSlot},
    primitive::{ConditionVariable, LockGuard, Mutex, ReadinessSignal},
};

#[cfg(test)]
mod test {
    use std::{io::Result, os::fd::AsRawFd as _};

    use crate::{driver::Reactor, sync::ReadinessSignal};

    const RING_ENTRIES: u32 = 64;
    const THREAD_COUNT: u32 = 8;

    #[test]
    fn thread_synchronization() -> Result<()> {
        let start = ReadinessSignal::new(THREAD_COUNT);
        let reactor = Reactor::initialize(RING_ENTRIES, None)?;

        std::thread::scope(|scope| {
            let mut threads = Vec::with_capacity(THREAD_COUNT.try_into().unwrap_or(usize::MAX));

            for _ in 0..THREAD_COUNT {
                let start = &start;
                let parent = reactor.as_raw_fd();

                threads.push(scope.spawn(move || {
                    let reactor = Reactor::initialize(RING_ENTRIES, Some(parent))?;

                    reactor.block_on(None, None, async {
                        start.arrive_and_wait(&reactor).await;
                    })
                }));
            }

            reactor.block_on(None, None, async {
                start.wait_ready(&reactor).await;
            })?;

            for thread in threads {
                thread.join().expect("worker threads shouldn't panic")?;
            }

            Ok(())
        })
    }
}
