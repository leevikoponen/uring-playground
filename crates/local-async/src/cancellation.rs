use std::{
    cell::{Cell, RefCell},
    task::{Poll, Waker},
};

use slab::Slab;

/// Minimal mechanism for cancellation, optimized for a small amount of waiters.
#[derive(Default)]
#[must_use]
pub struct CancellationToken {
    triggered: Cell<bool>,
    waiting: RefCell<Slab<Waker>>,
}

impl CancellationToken {
    /// Create a token without any preallocated capacity.
    pub const fn new() -> Self {
        Self {
            triggered: Cell::new(false),
            waiting: RefCell::new(Slab::new()),
        }
    }

    /// Create a token with initial capacity for at least this many
    /// wakers.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            triggered: Cell::new(false),
            waiting: RefCell::new(Slab::with_capacity(capacity)),
        }
    }

    /// Trigger any waiting futures.
    pub fn trigger(&self) {
        if self.triggered.replace(true) {
            return;
        }

        self.waiting.borrow_mut().drain().for_each(Waker::wake);
    }

    /// Wait for the token to be triggered.
    ///
    /// # Panics
    ///
    /// If internal assumptions are broken.
    pub async fn wait(&self) {
        let mut registration = scopeguard::guard(None, |mut state| {
            if let Some(key) = state.take() {
                // we don't care if the entry doesn't exist when dropping the future as they're
                // destructively iterated over when triggering the cancellation(s)
                let _ = self.waiting.borrow_mut().try_remove(key);
            }
        });

        std::future::poll_fn(move |context| {
            if self.triggered.get() {
                return Poll::Ready(());
            }

            let mut guard = self.waiting.borrow_mut();
            let Some(key) = *registration else {
                *registration = Some(guard.insert(context.waker().clone()));
                return Poll::Pending;
            };

            let current = guard
                .get_mut(key)
                .expect("handles should point to a registered waker slot");

            if !current.will_wake(context.waker()) {
                context.waker().clone_into(current);
            }

            Poll::Pending
        })
        .await;
    }
}

#[cfg(test)]
mod test {
    use futures_lite::future;

    use super::CancellationToken;

    #[test]
    fn wakes_waiting_task() {
        let token = CancellationToken::new();

        future::block_on(future::or(
            async {
                token.wait().await;
            },
            async {
                token.trigger();
                future::yield_now().await;
                unreachable!("trigger and yield should have caused return");
            },
        ));
    }

    #[test]
    fn no_spurious_wakeup() {
        let token = CancellationToken::new();

        future::block_on(future::or(
            async {
                token.wait().await;
                unreachable!("shouln't return without triggering");
            },
            async {
                future::yield_now().await;
            },
        ));
    }
}
