use std::{
    ops::{Deref, DerefMut},
    task::{Context, Waker},
};

/// Value that's associated with task to be notified on changes.
#[derive(Default)]
#[must_use]
pub struct NotifiedValue<T> {
    inner: T,
    task: Option<Waker>,
}

impl<T> NotifiedValue<T> {
    pub const fn new(inner: T) -> Self {
        Self { inner, task: None }
    }

    /// Register to be woken later.
    pub fn register(&mut self, context: &Context) {
        if !self
            .task
            .as_ref()
            .is_some_and(|current| current.will_wake(context.waker()))
        {
            self.task = Some(context.waker().clone());
        }
    }

    /// Wake the registered task if any.
    pub fn trigger(&mut self) {
        if let Some(waker) = self.task.take() {
            waker.wake();
        }
    }
}

impl<T> Deref for NotifiedValue<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> DerefMut for NotifiedValue<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
