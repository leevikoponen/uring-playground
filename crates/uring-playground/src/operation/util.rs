use std::{
    any::Any,
    pin::Pin,
    task::{Context, Poll},
};

use io_uring::{cqueue, squeue};
use pin_project::pin_project;

use crate::{
    driver::{OperationId, Reactor},
    operation::{Batch, Operation},
};

/// Adapter for converting operation output.
#[pin_project]
#[must_use]
pub struct MapOutput<O, F> {
    #[pin]
    operation: O,
    conversion: F,
}

impl<O, F> MapOutput<O, F> {
    pub const fn new(operation: O, conversion: F) -> Self {
        Self {
            operation,
            conversion,
        }
    }
}

// SAFETY: the internal operation promises safety
unsafe impl<T, O: Operation, F: FnMut(O::Output) -> T> Operation for MapOutput<O, F> {
    type Output = T;

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        self.project().operation.build_submission()
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        let this = self.project();

        // SAFETY: we control the submission
        (this.conversion)(unsafe { this.operation.handle_completion(entry) })
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        self.project().operation.take_allocations()
    }
}

// SAFETY: the internal operation promises safety
unsafe impl<T, O: Batch, F: FnMut(O::Output) -> T> Batch for MapOutput<O, F> {
    type Handle = O::Handle;
    type Output = T;

    fn first_operation(handle: Self::Handle) -> OperationId {
        O::first_operation(handle)
    }

    fn submit_entries(
        self: Pin<&mut Self>,
        reactor: &Reactor,
        context: Option<&Context>,
    ) -> Self::Handle {
        self.project().operation.submit_entries(reactor, context)
    }

    unsafe fn poll_progress(
        self: Pin<&mut Self>,
        handle: Self::Handle,
        reactor: &Reactor,
        context: &Context,
    ) -> Poll<Self::Output> {
        let this = self.project();

        // SAFETY: caller guarantees safety
        unsafe { this.operation.poll_progress(handle, reactor, context) }
            .map(|output| (this.conversion)(output))
    }

    fn drop_operations(self: Pin<&mut Self>, handle: Self::Handle, reactor: &Reactor) {
        self.project().operation.drop_operations(handle, reactor);
    }
}

/// Adapter for oneshot operations that captures the output internally.
#[pin_project]
#[must_use]
pub struct StashOutput<O: Operation> {
    #[pin]
    operation: O,
    output: Option<O::Output>,
}

impl<O: Operation> StashOutput<O> {
    pub const fn new(operation: O) -> Self {
        Self {
            operation,
            output: None,
        }
    }

    /// Check if there's output available.
    pub const fn has_output(&self) -> bool {
        self.output.is_some()
    }

    /// Consume the stored output.
    #[must_use]
    pub fn take_output(self: Pin<&mut Self>) -> Option<O::Output> {
        self.project().output.take()
    }
}

// SAFETY: the internal operation promises safety
unsafe impl<O: Operation> Operation for StashOutput<O> {
    type Output = ();

    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry {
        self.project().operation.build_submission()
    }

    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output {
        let this = self.project();
        assert!(
            this.output.is_none(),
            "oneshot operation shouldn't receive multiple completions"
        );

        // SAFETY: we control the submission
        *this.output = Some(unsafe { this.operation.handle_completion(entry) });
    }

    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>> {
        let this = self.project();
        if this.output.is_some() {
            return None;
        }

        this.operation.take_allocations()
    }
}
