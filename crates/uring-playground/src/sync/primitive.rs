use std::{
    cell::UnsafeCell,
    ops::{ControlFlow, Deref, DerefMut},
    sync::atomic::{AtomicU32, Ordering},
};

use crate::{
    driver::Reactor,
    operation::{FutexWait, FutexWake, Operation as _},
};

/// The error message when a futex operation fails.
///
/// # Notes
///
/// There's not really any reasonable situation that these can be handled,
/// especially given we're not supporting cancellation here.
const FUTEX_ERROR_MESSAGE: &str = "futex operations shouldn't fail and can't be reasonably handled";

/// Simple mutex implementation.
#[must_use]
pub struct Mutex<T> {
    flag: AtomicU32,
    value: UnsafeCell<T>,
}

// SAFETY: we have to trust our implementation
unsafe impl<T: Send> Send for Mutex<T> {}

// SAFETY: we have to trust our implementation
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    const STATE_UNLOCKED: u32 = 0;
    const STATE_LOCKED: u32 = 1;
    const STATE_CONTENDED: u32 = 2;
    const SPIN_ITERATIONS: usize = 100;

    /// Wrap the value around a mutex.
    pub const fn new(value: T) -> Self {
        Self {
            flag: AtomicU32::new(Self::STATE_UNLOCKED),
            value: UnsafeCell::new(value),
        }
    }

    /// Try and wait for the lock to be unlocked or contended.
    fn spin_loop(&self) -> u32 {
        let mut remaining = Self::SPIN_ITERATIONS;
        loop {
            let current = self.flag.load(Ordering::Relaxed);
            if current != Self::STATE_LOCKED || remaining == 0 {
                return current;
            }

            std::hint::spin_loop();
            remaining -= 1;
        }
    }

    /// Try to get into the unlocked state.
    fn try_transition_locked(&self) -> Result<u32, u32> {
        self.flag.compare_exchange(
            Self::STATE_UNLOCKED,
            Self::STATE_LOCKED,
            Ordering::Acquire,
            Ordering::Relaxed,
        )
    }

    /// Loop guard for the contended case.
    fn remains_contended(&self, state: u32) -> bool {
        if state == Self::STATE_CONTENDED {
            return true;
        }

        self.flag.swap(Self::STATE_CONTENDED, Ordering::Acquire) != Self::STATE_UNLOCKED
    }

    /// The contended case of getting unique access.
    async fn handle_contended(&self, reactor: &Reactor) {
        let mut state = self.spin_loop();
        if state == Self::STATE_UNLOCKED {
            match self.try_transition_locked() {
                Ok(_) => return,
                Err(current) => state = current,
            }
        }

        while self.remains_contended(state) {
            FutexWait::new(&self.flag, Self::STATE_CONTENDED)
                .build_oneshot(reactor)
                .await
                .expect(FUTEX_ERROR_MESSAGE);

            state = self.spin_loop();
        }
    }

    /// Wait for unique access.
    async fn lock(&self, reactor: &Reactor) {
        if self.try_transition_locked().is_err() {
            self.handle_contended(reactor).await;
        }
    }

    /// Release unique access away.
    async fn unlock(&self, reactor: &Reactor) {
        if self.flag.swap(Self::STATE_UNLOCKED, Ordering::Release) == Self::STATE_CONTENDED {
            FutexWake::new(&self.flag, 1)
                .build_oneshot(reactor)
                .await
                .expect(FUTEX_ERROR_MESSAGE);
        }
    }

    /// Acquire unique access to the value.
    pub async fn acquire(&self, reactor: &Reactor) -> LockGuard<'_, T> {
        self.lock(reactor).await;
        LockGuard { inner: self }
    }
}

/// Borrow guard for a locked mutex.
#[must_use]
pub struct LockGuard<'parent, T> {
    inner: &'parent Mutex<T>,
}

impl<T> LockGuard<'_, T> {
    /// Release access to the value.
    pub async fn release(self, reactor: &Reactor) {
        self.inner.unlock(reactor).await;
    }
}

impl<T> Deref for LockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: only exists while holding mutex
        unsafe { &*self.inner.value.get() }
    }
}

impl<T> DerefMut for LockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: only exists while holding mutex
        unsafe { &mut *self.inner.value.get() }
    }
}

/// Simple condition variable implementation.
#[derive(Default)]
#[must_use]
pub struct ConditionVariable {
    counter: AtomicU32,
}

impl ConditionVariable {
    /// Initialize a condition variable.
    pub const fn new() -> Self {
        Self {
            counter: AtomicU32::new(0),
        }
    }

    /// Notify waiting tasks.
    ///
    /// # Panics
    ///
    /// If internal operations fail.
    pub async fn notify(&self, reactor: &Reactor, count: u64) {
        self.counter.fetch_add(1, Ordering::Relaxed);
        FutexWake::new(&self.counter, count)
            .build_oneshot(reactor)
            .await
            .expect(FUTEX_ERROR_MESSAGE);
    }

    /// Wait to be notified.
    ///
    /// # Panics
    ///
    /// If internal operations fail.
    pub async fn wait<'lock, T>(
        &self,
        reactor: &Reactor,
        guard: LockGuard<'lock, T>,
    ) -> LockGuard<'lock, T> {
        let observed = self.counter.load(Ordering::Relaxed);
        guard.inner.unlock(reactor).await;

        FutexWait::new(&self.counter, observed)
            .build_oneshot(reactor)
            .await
            .expect(FUTEX_ERROR_MESSAGE);

        guard.inner.lock(reactor).await;
        guard
    }
}

/// Downward counter used to synchronize between threads.
#[must_use]
pub struct ReadinessSignal {
    counter: AtomicU32,
}

impl ReadinessSignal {
    /// Initialize the counter with the starting value.
    pub const fn new(count: u32) -> Self {
        Self {
            counter: AtomicU32::new(count),
        }
    }

    /// Decrement the counter and wake other threads if we're the last one,
    /// returning value determining if the other threads were ready.
    ///
    /// # Panics
    ///
    /// If internal operations fail.
    pub async fn count_down(&self, reactor: &Reactor) -> ControlFlow<()> {
        let previous = self.counter.fetch_sub(1, Ordering::SeqCst);
        if previous != 1 {
            return ControlFlow::Continue(());
        }

        FutexWake::new(&self.counter, u64::MAX)
            .build_oneshot(reactor)
            .await
            .expect(FUTEX_ERROR_MESSAGE);

        ControlFlow::Break(())
    }

    /// Wait until the other threads are finished.
    ///
    /// # Panics
    ///
    /// If internal operations fail.
    pub async fn wait_ready(&self, reactor: &Reactor) {
        loop {
            let current = self.counter.load(Ordering::SeqCst);
            if current == 0 {
                return;
            }

            FutexWait::new(&self.counter, current)
                .build_oneshot(reactor)
                .await
                .expect(FUTEX_ERROR_MESSAGE);
        }
    }

    /// Decrement the counter and wait until other threads are done.
    pub async fn arrive_and_wait(&self, reactor: &Reactor) {
        if self.count_down(reactor).await.is_continue() {
            self.wait_ready(reactor).await;
        }
    }
}
