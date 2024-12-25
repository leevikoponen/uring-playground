use std::{
    cell::RefCell,
    cmp::Ordering,
    task::{Poll, Waker},
    time::Instant,
};

use index_heap::PriorityQueue;

/// Internal state of a registered wakeup.
struct RegistrationEntry {
    instant: Instant,
    waker: Option<Waker>,
}

impl RegistrationEntry {
    /// Check if the waker will also wake the one attached to this entry.
    fn will_be_woken_by(&self, other: &Waker) -> bool {
        self.waker
            .as_ref()
            .is_some_and(|registered| other.will_wake(registered))
    }
}

impl Eq for RegistrationEntry {}

impl PartialEq for RegistrationEntry {
    fn eq(&self, other: &Self) -> bool {
        self.instant == other.instant
    }
}

impl Ord for RegistrationEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.instant.cmp(&other.instant)
    }
}

impl PartialOrd for RegistrationEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Queue of scheduled wakeups that can be triggered when known to be elapsed.
#[derive(Default)]
#[must_use]
pub struct WakeupQueue {
    state: RefCell<PriorityQueue<RegistrationEntry>>,
}

impl WakeupQueue {
    /// Create an instance without preallocated capacity.
    pub const fn new() -> Self {
        Self {
            state: RefCell::new(PriorityQueue::new()),
        }
    }

    /// Create an instance with preallocated capacity for at least this many
    /// registrations.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            state: RefCell::new(PriorityQueue::with_capacity(capacity)),
        }
    }

    /// Get when the next wakeup will happen.
    #[must_use]
    pub fn next_scheduled(&self) -> Option<Instant> {
        self.state
            .borrow()
            .head()
            .map(|registration| registration.instant)
    }

    /// Wait until the provided instant has been marked as elapsed.
    pub async fn sleep_until(&self, wakeup: Instant) {
        let mut registration = scopeguard::guard(None, |mut state| {
            if let Some(key) = state.take() {
                // we don't care if the entry doesn't exist when dropping as
                // getting triggered is represented as a removal anyways
                let _ = self.state.borrow_mut().remove(key);
            }
        });

        std::future::poll_fn(move |context| {
            let mut guard = self.state.borrow_mut();

            let Some(key) = *registration else {
                *registration = Some(guard.push(RegistrationEntry {
                    instant: wakeup,
                    waker: Some(context.waker().clone()),
                }));

                return Poll::Pending;
            };

            let Some(state) = guard.get_mut(key) else {
                return Poll::Ready(());
            };

            if !state.will_be_woken_by(context.waker()) {
                state.waker = Some(context.waker().clone());
            }

            Poll::Pending
        })
        .await;
    }

    /// Trigger all elapsed entries based on the current time.
    pub fn trigger_elapsed(&self, current: Instant) {
        self.state.borrow_mut().filter(|slot| {
            if slot.instant > current {
                return Some(slot);
            }

            if let Some(waker) = slot.remove().waker.take() {
                waker.wake();
            }

            None
        });
    }
}

#[cfg(test)]
mod test {
    use std::time::{Duration, Instant};

    use futures_lite::future;

    use super::WakeupQueue;

    const EXPECTED_ACCURACY: Duration = Duration::from_millis(10);
    const SLEEP_DURATION: Duration = Duration::from_millis(100);

    #[test]
    fn wakes_task_correctly() {
        let timer = WakeupQueue::new();
        future::block_on(future::or(
            async {
                let start = Instant::now();
                timer.sleep_until(start + SLEEP_DURATION).await;
                assert!(
                    start.elapsed().abs_diff(SLEEP_DURATION) < EXPECTED_ACCURACY,
                    "blocking sleep should be reasonably accurate"
                );
            },
            async {
                let duration = timer
                    .next_scheduled()
                    .and_then(|upcoming| upcoming.checked_duration_since(Instant::now()))
                    .expect("first future should have scheduled wakeup");

                std::thread::sleep(duration);
                timer.trigger_elapsed(Instant::now());

                future::yield_now().await;
                unreachable!("trigger and yield should have caused return");
            },
        ));
    }

    #[test]
    fn no_spurious_wakeup() {
        let timer = WakeupQueue::new();
        future::block_on(future::or(
            async {
                timer.sleep_until(Instant::now() + SLEEP_DURATION).await;
                unreachable!("shouln't return without triggering");
            },
            async {
                future::yield_now().await;
            },
        ));
    }
}
