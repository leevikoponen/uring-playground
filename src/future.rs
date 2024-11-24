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
pub struct SubmitAndWait<'reactor, B: Batch> {
    reactor: &'reactor RefCell<Reactor>,
    batch: B,
    handle: Option<B::Handle>,
}

impl<'reactor, B: Batch> SubmitAndWait<'reactor, B> {
    pub const fn new(reactor: &'reactor RefCell<Reactor>, batch: B) -> Self {
        Self {
            reactor,
            batch,
            handle: None,
        }
    }
}

impl<B> Future for SubmitAndWait<'_, B>
where
    B: Batch + Unpin,
    B::Handle: Copy + Unpin,
{
    type Output = B::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut reactor = this.reactor.borrow_mut();

        let handle = *this
            .handle
            .get_or_insert_with(|| this.batch.submit_entries(&mut reactor, Some(cx)));

        // SAFETY: we control the submission above
        unsafe {
            this.batch
                .poll_progress(handle, &mut reactor, cx)
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
