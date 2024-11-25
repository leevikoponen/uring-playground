use std::{
    any::Any,
    task::{Context, Poll},
};

use io_uring::{cqueue, squeue};

use crate::{
    operation::{Batch, Oneshot, Operation},
    reactor::{OperationId, Reactor},
};

/// Wrapper for [`Oneshot`] that captures the output internally.
#[must_use]
pub struct StashOutput<O: Oneshot> {
    operation: O,
    output: Option<O::Output>,
}

impl<O: Oneshot> StashOutput<O> {
    pub const fn new(operation: O) -> Self {
        Self {
            operation,
            output: None,
        }
    }

    #[must_use]
    pub const fn not_finished(&self) -> bool {
        self.output.is_none()
    }

    #[must_use]
    pub fn take_output(&mut self) -> Option<O::Output> {
        self.output.take()
    }
}

// SAFETY: the internal operation promises safety
unsafe impl<O: Oneshot> Operation for StashOutput<O> {
    type Output = ();

    fn build_submission(&mut self) -> squeue::Entry {
        self.operation.build_submission()
    }

    unsafe fn handle_completion(&mut self, entry: cqueue::Entry) -> Self::Output {
        assert!(self.output.is_none());

        // SAFETY: we control the submission
        unsafe {
            self.output = Some(self.operation.handle_completion(entry));
        }
    }

    fn take_required_allocations(&mut self) -> Option<Box<dyn Any>> {
        if self.output.is_some() {
            return None;
        }

        self.operation.take_required_allocations()
    }
}

// SAFETY: the internal operation is oneshot
unsafe impl<O: Oneshot> Oneshot for StashOutput<O> {}

/// Wrapper for operations that transforms the output.
#[must_use]
pub struct MapOutput<O, F> {
    operation: O,
    function: F,
}

impl<O, F> MapOutput<O, F> {
    pub const fn new(operation: O, function: F) -> Self {
        Self {
            operation,
            function,
        }
    }
}

// SAFETY: the internal operation promises safety
unsafe impl<O, F, T> Operation for MapOutput<O, F>
where
    O: Operation,
    F: FnMut(O::Output) -> T,
{
    type Output = T;

    fn build_submission(&mut self) -> squeue::Entry {
        self.operation.build_submission()
    }

    unsafe fn handle_completion(&mut self, entry: cqueue::Entry) -> Self::Output {
        (self.function)(self.operation.handle_completion(entry))
    }

    fn take_required_allocations(&mut self) -> Option<Box<dyn Any>> {
        self.operation.take_required_allocations()
    }
}

// SAFETY: the internal operation is oneshot
unsafe impl<O, F, T> Oneshot for MapOutput<O, F>
where
    O: Oneshot,
    F: FnMut(O::Output) -> T,
{
}

// SAFETY: the internal operation promises safety
unsafe impl<B, F, T> Batch for MapOutput<B, F>
where
    B: Batch,
    F: FnMut(B::Output) -> T,
{
    type Handle = B::Handle;

    type Output = T;

    fn submit_entries(&mut self, reactor: &mut Reactor, context: Option<&Context>) -> Self::Handle {
        self.operation.submit_entries(reactor, context)
    }

    unsafe fn poll_progress(
        &mut self,
        handle: Self::Handle,
        reactor: &mut Reactor,
        context: &Context,
    ) -> Poll<Self::Output> {
        self.operation
            .poll_progress(handle, reactor, context)
            .map(|output| (self.function)(output))
    }

    fn drop_operations(&mut self, handle: Self::Handle, reactor: &mut Reactor) {
        self.operation.drop_operations(handle, reactor);
    }
}

/// Singular [`Oneshot`] acting as a [`Batch`].
#[must_use]
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
