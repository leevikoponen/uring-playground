use std::{cell::RefCell, task::Poll};

use crate::notify::NotifiedValue;

/// State for tracking value exchanges.
pub struct ExchangeState<T> {
    current: Option<T>,
    closed: bool,
    reader: NotifiedValue<()>,
    writer: NotifiedValue<()>,
}

/// Closable slot for moving values between tasks one at a time.
#[must_use]
pub struct ValueExchange<T> {
    inner: RefCell<ExchangeState<T>>,
}

impl<T> ValueExchange<T> {
    pub const fn new() -> Self {
        Self {
            inner: RefCell::new(ExchangeState {
                current: None,
                closed: false,
                reader: NotifiedValue::new(()),
                writer: NotifiedValue::new(()),
            }),
        }
    }

    /// Mark the channel as closed.
    pub fn close(&self) {
        let mut guard = self.inner.borrow_mut();
        guard.closed = true;
        guard.reader.trigger();
        guard.writer.trigger();
    }

    /// Send a value to the other side unless closed.
    ///
    /// # Errors
    ///
    /// If the channel has already been closed.
    ///
    /// # Panics
    ///
    /// If the future is polled again after it's ready.
    pub async fn send(&self, value: T) -> Result<(), T> {
        let mut value = Some(value);
        std::future::poll_fn(|context| {
            let mut guard = self.inner.borrow_mut();
            if guard.closed {
                Poll::Ready(Err(value
                    .take()
                    .expect("futures shouldn't be polled after finishing")))
            } else if guard.current.is_none() {
                guard.current = Some(
                    value
                        .take()
                        .expect("futures shouldn't be polled after finishing"),
                );

                guard.reader.trigger();
                Poll::Ready(Ok(()))
            } else {
                guard.writer.register(context);
                Poll::Pending
            }
        })
        .await
    }

    /// Receive a value from the other side unless closed.
    #[must_use]
    pub async fn receive(&self) -> Option<T> {
        std::future::poll_fn(|context| {
            let mut guard = self.inner.borrow_mut();
            if let Some(value) = guard.current.take() {
                guard.writer.trigger();
                Poll::Ready(Some(value))
            } else if guard.closed {
                Poll::Ready(None)
            } else {
                guard.reader.register(context);
                Poll::Pending
            }
        })
        .await
    }
}

impl<T> Default for ValueExchange<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod test {
    use futures_lite::future;

    use super::ValueExchange;

    #[test]
    fn passes_values() {
        let exchange = ValueExchange::new();
        future::block_on(future::or(
            async {
                exchange
                    .send(1)
                    .await
                    .expect("exchange shouldn't become closed on it's own");

                future::yield_now().await;
            },
            async {
                assert_eq!(
                    exchange.receive().await,
                    Some(1),
                    "exchange should receive value correctly"
                );
            },
        ));
    }

    #[test]
    fn sends_immediately_when_empty() {
        let exchange = ValueExchange::new();
        future::block_on(async {
            assert_eq!(
                future::poll_once(exchange.send(())).await,
                Some(Ok(())),
                "exchange should not block when sending first value"
            );
        });
    }

    #[test]
    fn keeps_waiting_as_needed() {
        let exchange = ValueExchange::<()>::new();
        future::block_on(future::or(
            async {
                let _ = exchange.receive().await;
                unreachable!("receive shouldn't return if nothing is sent")
            },
            async {
                future::yield_now().await;
            },
        ));
    }
}
