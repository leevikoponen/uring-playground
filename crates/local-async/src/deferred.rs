use std::{cell::RefCell, task::Poll};

use crate::notify::NotifiedValue;

/// Deferred value that gets set in the future.
#[must_use]
pub struct DeferredValue<T> {
    inner: RefCell<NotifiedValue<Option<T>>>,
}

impl<T> DeferredValue<T> {
    pub const fn new() -> Self {
        Self {
            inner: RefCell::new(NotifiedValue::new(None)),
        }
    }

    /// Set the value and wake the waiting task if one exists.
    pub fn set(&self, value: T) {
        let mut guard = self.inner.borrow_mut();
        guard.replace(value);
        guard.trigger();
    }

    /// Wait for a value to be set.
    pub async fn get(&self) -> T {
        std::future::poll_fn(|context| {
            let mut guard = self.inner.borrow_mut();
            if let Some(value) = guard.take() {
                return Poll::Ready(value);
            }

            guard.register(context);
            Poll::Pending
        })
        .await
    }
}

impl<T> Default for DeferredValue<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod test {
    use futures_lite::future;

    use super::DeferredValue;

    #[test]
    fn notifies_on_insert() {
        let value = DeferredValue::new();
        future::block_on(future::or(
            async {
                value.get().await;
            },
            async {
                value.set(());
                future::yield_now().await;
                unreachable!("getting value should have completed by our set");
            },
        ));
    }

    #[test]
    fn keeps_waiting_as_needed() {
        let value = DeferredValue::new();
        future::block_on(future::or(
            async {
                let () = value.get().await;
                unreachable!("get shouldn't return if the value doesn't get set")
            },
            async {
                future::yield_now().await;
            },
        ));
    }
}
