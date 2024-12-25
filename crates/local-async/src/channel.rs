use std::{cell::RefCell, mem::MaybeUninit, task::Poll};

use crate::notify::NotifiedValue;

/// Internal mutable state of a channel.
struct ChannelState<T> {
    buffer: Box<[MaybeUninit<T>]>,
    closed: bool,
    reader: NotifiedValue<usize>,
    writer: NotifiedValue<usize>,
}

impl<T> ChannelState<T> {
    /// Use an internal index to get a reference to the corresponding slot.
    #[expect(clippy::indexing_slicing, reason = "the index is effectively clamped")]
    fn get_mut(&mut self, index: usize) -> &mut MaybeUninit<T> {
        // I'll have to admit that I don't even remember where this trick originates
        // from, but it has generally worked well for me in the past during various
        // situations, including experimenting with lock free programming
        //
        // the implementation essentially uses infinitely growing indices to keep track
        // of positions and it turns out that it's very easy to avoid having to keep
        // them within actual bounds by just clamping the values with bit trickery here
        &mut self.buffer[index & (self.buffer.len() - 1)]
    }

    /// Try to pop a value from the queue unless it's empty.
    fn try_pop(&mut self) -> Result<T, ()> {
        if *self.reader == *self.writer {
            return Err(());
        }

        let entry = std::mem::replace(self.get_mut(*self.reader), MaybeUninit::uninit());
        *self.reader = self.reader.wrapping_add(1);

        // SAFETY: index should have pointed to a previously pushed and unconsumed entry
        Ok(unsafe { entry.assume_init() })
    }

    /// Try to push a value into the queue, retuning it if there wasn't space.
    fn try_push(&mut self, value: T) -> Result<(), T> {
        if self.writer.wrapping_sub(*self.reader) == self.buffer.len() {
            return Err(value);
        }

        self.get_mut(*self.writer).write(value);
        *self.writer = self.writer.wrapping_add(1);

        Ok(())
    }
}

/// Simple bounded ring buffer for moving items between futures.
#[must_use]
pub struct BoundedChannel<T> {
    state: RefCell<ChannelState<T>>,
}

impl<T> BoundedChannel<T> {
    /// Create a new channel with the given capacity.
    ///
    /// # Panics
    ///
    /// If the capacity is not an power of two.
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity.is_power_of_two(),
            "the ring buffer implementation relies upon capacities being powers of two"
        );

        Self {
            state: RefCell::new(ChannelState {
                buffer: Box::new_uninit_slice(capacity),
                closed: false,
                reader: NotifiedValue::new(0),
                writer: NotifiedValue::new(0),
            }),
        }
    }

    /// Mark the channel as closed.
    pub fn close(&self) {
        let mut guard = self.state.borrow_mut();
        guard.closed = true;
        guard.reader.trigger();
        guard.writer.trigger();
    }

    /// Pop a value from the channel.
    pub async fn pop(&self) -> Option<T> {
        std::future::poll_fn(|context| {
            let mut guard = self.state.borrow_mut();
            if let Ok(value) = guard.try_pop() {
                guard.writer.trigger();
                return Poll::Ready(Some(value));
            }

            if guard.closed {
                return Poll::Ready(None);
            }

            guard.reader.register(context);
            Poll::Pending
        })
        .await
    }

    /// Push a value into the channel.
    ///
    /// # Errors
    ///
    /// If the channel has been closed.
    ///
    /// # Panics
    ///
    /// If the future is polled after finishing.
    pub async fn push(&self, value: T) -> Result<(), T> {
        let mut value = Some(value);
        std::future::poll_fn(|context| {
            let mut guard = self.state.borrow_mut();
            let Err(returned) = guard.try_push(
                value
                    .take()
                    .expect("futures shouldn't be polled after finishing"),
            ) else {
                guard.reader.trigger();
                return Poll::Ready(Ok(()));
            };

            if guard.closed {
                return Poll::Ready(Err(value
                    .take()
                    .expect("futures shouldn't be polled after finishing")));
            }

            guard.writer.register(context);
            value = Some(returned);
            Poll::Pending
        })
        .await
    }
}

#[cfg(test)]
mod test {
    use futures_lite::future;

    use super::BoundedChannel;

    const CHANNEL_SIZE: usize = 4;
    const TEST_ITEMS: &[i32] = &[0, 1, 2, 3, 4, 5];

    const _: () = assert!(
        TEST_ITEMS.len() > CHANNEL_SIZE,
        "shouldn't be using so few example items as to fit all within capacity"
    );

    #[test]
    fn transfers_entries_correctly() {
        let channel = BoundedChannel::new(CHANNEL_SIZE);
        future::block_on(future::or(
            async {
                for value in TEST_ITEMS {
                    assert_eq!(
                        channel.pop().await,
                        Some(value),
                        "channel should return same values as was pushed into it"
                    );
                }
            },
            async {
                for value in TEST_ITEMS {
                    channel
                        .push(value)
                        .await
                        .expect("channel shouldn't randomly become closed");
                }
            },
        ));
    }

    #[test]
    fn no_waiting_within_capacity() {
        let channel = BoundedChannel::new(CHANNEL_SIZE);
        future::block_on(future::or(
            async {
                for (index, value) in TEST_ITEMS.iter().enumerate() {
                    if index != 0 && index.is_multiple_of(CHANNEL_SIZE) {
                        future::yield_now().await;
                    }

                    assert_eq!(
                        future::poll_once(channel.push(value)).await,
                        Some(Ok(())),
                        "push when yielding before hitting capacity should return instantly"
                    );
                }
            },
            async {
                for (index, value) in TEST_ITEMS.iter().enumerate() {
                    if index != 0 && index.is_multiple_of(CHANNEL_SIZE) {
                        future::yield_now().await;
                    }

                    assert_eq!(
                        future::poll_once(channel.pop()).await,
                        Some(Some(value)),
                        "pop when yielding before consuming all entries should return instantly"
                    );
                }
            },
        ));
    }
}
