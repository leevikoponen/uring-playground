//! Naive synchronization primitives implemented on top of asynchronous futex
//! operations.
//!
//! The implementations are adapted from memory of example algorithms shown on
//! the lecture slides of a concurrency course and might very possibly not be
//! entirely correct.
use std::{
    cell::{RefCell, UnsafeCell},
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicU32, Ordering},
};

use crate::{
    operation::{Batch as _, FutexWait, FutexWake, Operation as _},
    reactor::Reactor,
};

/// Minimal asynchronous semaphore implementation.
#[must_use]
pub struct Semaphore {
    count: AtomicU32,
}

impl Semaphore {
    pub const fn new(value: u32) -> Self {
        Self {
            count: AtomicU32::new(value),
        }
    }

    pub async fn aqquire(&self, reactor: &RefCell<Reactor>) {
        loop {
            let count = self.count.load(Ordering::SeqCst);
            if count == 0 {
                FutexWait::new(&self.count, count)
                    .into_batch()
                    .build_submission(reactor)
                    .await;

                continue;
            }

            let result = self.count.compare_exchange_weak(
                count,
                count - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );

            if result.is_ok() {
                return;
            }
        }
    }

    pub async fn release(&self, reactor: &RefCell<Reactor>) {
        if self.count.fetch_add(1, Ordering::SeqCst) == 0 {
            FutexWake::new(&self.count, 1)
                .into_batch()
                .build_submission(reactor)
                .await;
        }
    }
}

/// Minimal asynchronous mutex implementation.
#[must_use]
pub struct Mutex<T> {
    state: AtomicU32,
    value: UnsafeCell<T>,
}

impl<T> Mutex<T> {
    const STATE_UNLOCKED: u32 = 0;
    const STATE_LOCKED: u32 = 1;

    pub const fn new(value: T) -> Self {
        Self {
            state: AtomicU32::new(Self::STATE_UNLOCKED),
            value: UnsafeCell::new(value),
        }
    }

    async fn lock(&self, reactor: &RefCell<Reactor>) {
        loop {
            let result = self.state.compare_exchange_weak(
                Self::STATE_UNLOCKED,
                Self::STATE_LOCKED,
                Ordering::Acquire,
                Ordering::Relaxed,
            );

            if result.is_ok() {
                return;
            }

            FutexWait::new(&self.state, 1)
                .into_batch()
                .build_submission(reactor)
                .await;
        }
    }

    async fn unlock(&self, reactor: &RefCell<Reactor>) {
        if self.state.swap(Self::STATE_UNLOCKED, Ordering::Release) == Self::STATE_LOCKED {
            FutexWake::new(&self.state, 1)
                .into_batch()
                .build_submission(reactor)
                .await;
        }
    }

    pub async fn aqquire(&self, reactor: &RefCell<Reactor>) -> LockGuard<'_, T> {
        self.lock(reactor).await;
        LockGuard { inner: self }
    }
}

// SAFETY: we just have to trust our implementation
unsafe impl<T: Send> Send for Mutex<T> {}

// SAFETY: we just have to trust our implementation
unsafe impl<T: Send> Sync for Mutex<T> {}

/// Scope guard like utility representing access to a mutex.
///
/// # Warning
///
/// This guard must be manually released, the internal mutex will NOT be
/// released otherwise, leading any other calls to [`Mutex::aqquire`] getting
/// stuck forever.
pub struct LockGuard<'mutex, T> {
    inner: &'mutex Mutex<T>,
}

impl<T> LockGuard<'_, T> {
    pub async fn release(self, reactor: &RefCell<Reactor>) {
        self.inner.unlock(reactor).await;
    }
}

impl<T> Deref for LockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: this type can only be constructed by locking a mutex
        unsafe { &*self.inner.value.get() }
    }
}

impl<T> DerefMut for LockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: this type can only be constructed by locking a mutex
        unsafe { &mut *self.inner.value.get() }
    }
}

/// Minimal condition variable implementation.
#[must_use]
pub struct ConditionVariable {
    futex: AtomicU32,
}

impl ConditionVariable {
    #[expect(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {
            futex: AtomicU32::new(0),
        }
    }

    pub async fn wait<T>(&self, reactor: &RefCell<Reactor>, guard: LockGuard<'_, T>) {
        self.futex.fetch_add(1, Ordering::SeqCst);

        guard.inner.unlock(reactor).await;

        FutexWait::new(&self.futex, 1)
            .into_batch()
            .build_submission(reactor)
            .await;

        guard.inner.lock(reactor).await;
    }

    pub async fn notify_one(&self, reactor: &RefCell<Reactor>) {
        self.futex.fetch_sub(1, Ordering::SeqCst);

        FutexWake::new(&self.futex, 1)
            .into_batch()
            .build_submission(reactor)
            .await;
    }

    pub async fn notify_all(&self, reactor: &RefCell<Reactor>) {
        let waiters = self.futex.swap(0, Ordering::SeqCst);
        if waiters == 0 {
            return;
        }

        FutexWake::new(&self.futex, waiters)
            .into_batch()
            .build_submission(reactor)
            .await;
    }
}
