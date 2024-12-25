use std::collections::VecDeque;

use futures_lite::future;

use crate::{
    driver::Reactor,
    sync::{ConditionVariable, Mutex},
};

/// Concurrent notified slot that can be set by another thread.
pub struct ConcurrentSlot<T> {
    storage: Mutex<Option<T>>,
    ready: ConditionVariable,
}

impl<T> ConcurrentSlot<T> {
    /// Create a new empty slot.
    pub const fn new() -> Self {
        Self {
            storage: Mutex::new(None),
            ready: ConditionVariable::new(),
        }
    }

    /// Set the value and notify up the waiting thread.
    pub async fn set(&self, reactor: &Reactor, value: T) -> Option<T> {
        let mut guard = self.storage.acquire(reactor).await;
        let previous = guard.replace(value);

        future::zip(guard.release(reactor), self.ready.notify(reactor, 1)).await;
        previous
    }

    /// Wait for the value to be set by another thread.
    pub async fn get(&self, reactor: &Reactor) -> T {
        let mut guard = self.storage.acquire(reactor).await;
        loop {
            if let Some(item) = guard.take() {
                guard.release(reactor).await;
                return item;
            }

            guard = self.ready.wait(reactor, guard).await;
        }
    }
}

impl<T> Default for ConcurrentSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Concurrent queue that's pretty much the textbook naive implementation.
#[must_use]
pub struct ConcurrentQueue<T> {
    storage: Mutex<VecDeque<T>>,
    full: ConditionVariable,
    empty: ConditionVariable,
}

impl<T> ConcurrentQueue<T> {
    /// Initialize a queue with the given length bound.
    pub fn new(capacity: usize) -> Self {
        Self {
            storage: Mutex::new(VecDeque::with_capacity(capacity)),
            full: ConditionVariable::new(),
            empty: ConditionVariable::new(),
        }
    }

    /// Push a item to the queue.
    pub async fn push(&self, reactor: &Reactor, value: T) {
        let mut guard = self.storage.acquire(reactor).await;
        while guard.len() == guard.capacity() {
            guard = self.full.wait(reactor, guard).await;
        }

        guard.push_back(value);
        future::zip(guard.release(reactor), self.empty.notify(reactor, 1)).await;
    }

    /// Read an item from the queue.
    pub async fn pop(&self, reactor: &Reactor) -> T {
        let mut guard = self.storage.acquire(reactor).await;
        loop {
            if let Some(item) = guard.pop_front() {
                future::zip(guard.release(reactor), self.full.notify(reactor, 1)).await;
                return item;
            }

            guard = self.empty.wait(reactor, guard).await;
        }
    }
}
