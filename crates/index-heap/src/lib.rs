//! Minimal priority queue for situations requiring `O(1)` secondary direct
//! access to individual entries.
use std::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use slab::Slab;

/// Index pointing to a value on the queue.
pub struct EntryId<T> {
    inner: usize,
    marker: PhantomData<T>,
}

impl<T> Clone for EntryId<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for EntryId<T> {}

/// Priority queue in ascending order implemented as an binary min-heap.
#[must_use]
pub struct PriorityQueue<T> {
    storage: Slab<T>,
    queue: Vec<usize>,
}

impl<T> PriorityQueue<T> {
    /// Create an instance without preallocated capacity.
    pub const fn new() -> Self {
        Self {
            storage: Slab::new(),
            queue: Vec::new(),
        }
    }

    /// Create an instance with initial capacity for at least this many
    /// entries.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            storage: Slab::with_capacity(capacity),
            queue: Vec::with_capacity(capacity),
        }
    }

    /// Get a reference to the first item in the queue.
    #[must_use]
    pub fn head(&self) -> Option<&T> {
        let index = self.queue.first().copied()?;
        let value = self.storage.get(index)?;

        Some(value)
    }

    /// Get mutable access to an entry.
    #[must_use]
    pub fn get_mut(&mut self, index: EntryId<T>) -> Option<&mut T> {
        self.storage.get_mut(index.inner)
    }

    /// Remove a value from the queue.
    #[must_use]
    pub fn remove(&mut self, index: EntryId<T>) -> Option<T> {
        let (offset, _) = self
            .queue
            .iter()
            .enumerate()
            .find(|&(_, &current)| current == index.inner)?;

        self.queue.remove(offset);
        self.storage.try_remove(index.inner)
    }
}

impl<T: Ord> PriorityQueue<T> {
    // Get a reference to an entry through a slot in the queue.
    fn resolve_position(&self, position: usize) -> Option<&T> {
        self.queue
            .get(position)
            .and_then(|&index| self.storage.get(index))
    }

    /// Rebuild the internal binary heap before the index.
    fn rebuild_downward(&mut self, mut position: usize) {
        while position > 0 {
            let parent = (position - 1) / 2;
            if self.resolve_position(parent) < self.resolve_position(position) {
                break;
            }

            self.queue.swap(position, parent);
            position = parent;
        }
    }

    /// Rebuild the internal binary heap starting at the index.
    fn rebuild_upward(&mut self, mut position: usize) {
        let length = self.queue.len();
        loop {
            let left = 2 * position + 1;
            let right = 2 * position + 2;
            let mut min = position;

            if left < length && self.resolve_position(left) < self.resolve_position(min) {
                min = left;
            }

            if right < length && self.resolve_position(right) < self.resolve_position(min) {
                min = right;
            }

            if min == position {
                break;
            }

            self.queue.swap(position, min);
            position = min;
        }
    }

    /// Take out the first value from the queue.
    pub fn pop(&mut self) -> Option<T> {
        let length = self.queue.len();
        if length == 0 {
            return None;
        }

        self.queue.swap(0, length - 1);
        let index = self.queue.pop()?;
        let value = self.storage.try_remove(index)?;

        if length > 1 {
            self.rebuild_upward(0);
        }

        Some(value)
    }

    /// Push an item to the queue.
    pub fn push(&mut self, value: T) -> EntryId<T> {
        let inner = self.storage.insert(value);
        self.queue.push(inner);
        self.rebuild_downward(self.queue.len() - 1);

        EntryId {
            inner,
            marker: PhantomData,
        }
    }

    /// Iterate over entries to choose which to retain.
    pub fn filter(&mut self, condition: impl Fn(EntrySlot<'_, T>) -> Option<EntrySlot<'_, T>>) {
        self.queue.retain(|&index| {
            let entry = EntrySlot {
                storage: &mut self.storage,
                index,
            };

            condition(entry).is_some()
        });

        self.rebuild_downward(self.queue.len().saturating_sub(1));
    }
}

impl<T> Default for PriorityQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Temporary mutable handle to a queue entry.
#[must_use]
pub struct EntrySlot<'container, T> {
    storage: &'container mut Slab<T>,
    index: usize,
}

impl<T> EntrySlot<'_, T> {
    /// Remove the entry being iterated over.
    ///
    /// # Panics
    ///
    /// If the implementation is incorrect.
    #[must_use]
    pub fn remove(self) -> T {
        self.storage
            .try_remove(self.index)
            .expect("should not have constructed a handle to vacant entry")
    }
}

impl<T> Deref for EntrySlot<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.storage
            .get(self.index)
            .expect("entry should be constructed with valid index")
    }
}

impl<T> DerefMut for EntrySlot<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.storage
            .get_mut(self.index)
            .expect("entry should be constructed with valid index")
    }
}

#[cfg(test)]
mod test {
    use super::PriorityQueue;

    const TEST_VALUES: &[i32] = &[-46, -855, 740, 442, -306, -645, -374, 427, -210, 190];

    fn build_ordered_and_heap<T: Ord + Copy>(values: &[T]) -> (Vec<T>, PriorityQueue<T>) {
        let mut heap = PriorityQueue::with_capacity(values.len());
        for &item in values {
            heap.push(item);
        }

        let mut values = Vec::from(values);
        values.sort_unstable();

        (values, heap)
    }

    #[test]
    fn ordered_correctly() {
        let (expected, mut heap) = build_ordered_and_heap(TEST_VALUES);

        for value in expected.iter().copied() {
            assert_eq!(heap.pop(), Some(value));
        }
    }

    #[test]
    fn filter_keeps_ordering() {
        let (expected, mut heap) = build_ordered_and_heap(TEST_VALUES);

        heap.filter(|entry| entry.is_positive().then_some(entry));

        for value in expected.iter().copied().filter(|value| value.is_positive()) {
            assert_eq!(heap.pop(), Some(value));
        }
    }
}
