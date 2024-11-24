//! Implementation for using a [`Reactor`] to drive operations with
//! `async`/`await`.
use std::{
    cell::RefCell,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use crate::{batch::Batch, reactor::Reactor};

/// Future for submitting and waiting for a [`Batch`] to complete.
#[must_use]
pub struct SubmitAndWait<'a, B: Batch> {
    reactor: &'a RefCell<Reactor>,
    batch: B,
    handle: Option<B::Handle>,
}

impl<'a, B: Batch> SubmitAndWait<'a, B> {
    pub const fn new(reactor: &'a RefCell<Reactor>, batch: B) -> Self {
        Self {
            reactor,
            batch,
            handle: None,
        }
    }
}

impl<B: Batch> Future for SubmitAndWait<'_, B> {
    type Output = B::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut reactor = this.reactor.borrow_mut();

        let handle = *this
            .handle
            .get_or_insert_with(|| this.batch.submit_entries(&mut reactor, Some(context)));

        // SAFETY: we control the submission above
        unsafe {
            this.batch
                .poll_progress(handle, &mut reactor, context)
                .map(|output| {
                    this.handle = None;
                    output
                })
        }
    }
}

impl<B: Batch> Drop for SubmitAndWait<'_, B> {
    fn drop(&mut self) {
        let mut reactor = self.reactor.borrow_mut();

        if let Some(handle) = self.handle.take() {
            self.batch.drop_operations(handle, &mut reactor);
        }
    }
}
