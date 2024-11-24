use std::task::{Context, Poll};

use crate::{
    batch::Batch,
    operation::Oneshot,
    reactor::{OperationId, Reactor},
};

/// Singular [`Oneshot`] acting as a [`Batch`].
pub struct Single<O> {
    inner: O,
}

impl<O> Single<O> {
    pub const fn new(inner: O) -> Self {
        Self { inner }
    }
}

// SAFETY: the safety requirements are identical
unsafe impl<O: Oneshot> Batch for Single<O> {
    type Handle = OperationId;
    type Output = O::Output;

    fn submit_entries(&mut self, reactor: &mut Reactor, context: Option<&Context>) -> Self::Handle {
        // SAFETY: operation implementations guarantee safety
        unsafe { reactor.queue_submission(self.inner.build_submission(), context) }
    }

    unsafe fn poll_progress(
        &mut self,
        handle: Self::Handle,
        reactor: &mut Reactor,
        context: &Context,
    ) -> Poll<Self::Output> {
        reactor.poll_completion(handle, context).map(|entry| {
            // SAFETY: caller guarantees that we control the submission
            unsafe { self.inner.handle_completion(entry) }
        })
    }

    fn drop_operations(&mut self, handle: Self::Handle, reactor: &mut Reactor) {
        reactor.ignore_operation(handle, self.inner.take_required_allocations());
    }
}
