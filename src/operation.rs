use std::{
    any::Any,
    cell::RefCell,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use io_uring::{
    cqueue,
    squeue::{self, Flags},
};

use crate::reactor::{OperationId, Reactor};

/// Abstract representation of a singular `io_uring` operation.
///
/// # Safety
///
/// Implementations must ensure that parameters like mutable buffers are kept
/// alive for the entire duration of the operation, like through yielding them
/// from [`Operation::take_required_allocations`].
pub unsafe trait Operation: Unpin {
    /// What this operation ultimately produces.
    type Output: Unpin;

    /// Build a submission that represents this operation.
    fn build_submission(&mut self) -> squeue::Entry;

    /// Process this operation's completion.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the entry corresponds to the submission from
    /// [`Oneshot::build_submission`].
    unsafe fn handle_completion(&mut self, entry: cqueue::Entry) -> Self::Output;

    /// Take away allocated values that have to live for the duration of the
    /// operation instead of just until the submission has been made.
    ///
    /// This allows the operation struct to be safely dropped as long as the
    /// caller ensures these values get stashed somewhere.
    #[must_use = "required allocations must be stashed away somewhere"]
    fn take_required_allocations(&mut self) -> Option<Box<dyn Any>>;
}

/// [`Operation`] that guarantees to only produce one completion entry.
///
/// # Safety
///
/// The same requirements as [`Operation`] apply.
pub unsafe trait Oneshot: Operation {}

/// Wrapper for [`Oneshot`] that captures the output internally.
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

    pub const fn not_finished(&self) -> bool {
        self.output.is_none()
    }

    #[must_use]
    pub fn take_output(&mut self) -> Option<O::Output> {
        self.output.take()
    }
}

/// SAFETY: the internal operation promises safety
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

/// SAFETY: the internal operation is oneshot
unsafe impl<O: Oneshot> Oneshot for StashOutput<O> {}

/// Abstract representation of multiple [`Oneshot`] operations.
///
/// # Safety
///
/// The same requirements as [`Operation`] apply.
#[must_use = "operations do nothing unless submitted"]
pub unsafe trait Batch: Unpin {
    /// Handle to store information about submitted entries.
    ///
    /// This should allow for working with more than just one entry, such as
    /// with linked operations.
    type Handle: Copy + Unpin;

    /// What this operation ultimately produces.
    type Output: Unpin;

    /// Submit entries onto the specified reactor.
    #[must_use = "submission handle should be stored somewhere"]
    fn submit_entries(&mut self, reactor: &mut Reactor, context: Option<&Context>) -> Self::Handle;

    /// Poll for progress on the operations.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the handle originates from calling
    /// [`Operation::submit_entries`].
    unsafe fn poll_progress(
        &mut self,
        handle: Self::Handle,
        reactor: &mut Reactor,
        context: &Context,
    ) -> Poll<Self::Output>;

    /// Mark operations as ignored as a cancellation step in case of drop.
    fn drop_operations(&mut self, handle: Self::Handle, reactor: &mut Reactor);
}

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

/// Helper macro to consume an identifier token but actually use something else.
macro_rules! replace_ident {
    ($_:ident, $with:ident) => {
        $with
    };
}

/// Helper macro to define wrapper structs for linking [`Oneshot`] operations.
macro_rules! define_link_structs {
    (
        $(
            $(#[$attribute:meta])*
            $struct:ident { $($field:ident: $kind:ident),* }
        )*
    ) => {
        $(
            $(#[$attribute])*
            pub struct $struct<$($kind: Oneshot),*> { $($field: StashOutput<$kind>),* }

            impl<$($kind: Oneshot),*> $struct<$($kind),*> {
                pub const fn new($($field: $kind),*) -> Self {
                    Self { $( $field: StashOutput::new($field) ),* }
                }
            }

            // SAFETY: the safety requirements are identical
            unsafe impl<$($kind: Oneshot),*> Batch for $struct<$($kind),*> {
                type Handle = ($(replace_ident!($kind, OperationId),)*);
                type Output = ($($kind::Output,)*);

                fn submit_entries(
                    &mut self,
                    reactor: &mut Reactor,
                    context: Option<&Context>,
                ) -> Self::Handle {
                    let mut entries = [$(self.$field.build_submission()),*].into_iter();

                    $(
                        let $field = {
                            let entry = entries.next().unwrap();
                            let entry = if entries.len() != 0 {
                                entry.flags(Flags::IO_LINK)
                            } else {
                                entry
                            };

                            // SAFETY: operation implementations guarantee safety
                            unsafe { reactor.queue_submission(entry, context) }
                        };
                    )*

                    ($($field,)*)
                }

                unsafe fn poll_progress(
                    &mut self,
                    ($($field),*): Self::Handle,
                    reactor: &mut Reactor,
                    context: &Context,
                ) -> Poll<Self::Output> {
                    $(
                        if self.$field.not_finished() {
                            let output = reactor.poll_completion($field, context).map(|entry| {
                                // SAFETY: caller guarantees that we control the submission
                                unsafe { self.$field.handle_completion(entry) }
                            });

                            if output.is_pending() {
                                return Poll::Pending;
                            }
                        }
                    )*

                    Poll::Ready(($(self.$field.take_output().unwrap()),*))
                }

                fn drop_operations(&mut self, ($($field),*): Self::Handle, reactor: &mut Reactor) {
                    $(reactor.ignore_operation($field, self.$field.take_required_allocations());)*
                }
            }
        )*
    };
}

define_link_structs! {
    Link2 { first: A, second: B }
    Link3 { first: A, second: B, third: C }
    Link4 { first: A, second: B, third: C, fourth: D }
    Link5 { first: A, second: B, third: C, fourth: D, fifth: F }
}

/// Future for submitting and waiting for a [`Batch`] to complete.
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

        let handle = this
            .handle
            .get_or_insert_with(|| this.batch.submit_entries(&mut reactor, Some(context)));

        unsafe {
            this.batch
                .poll_progress(*handle, &mut reactor, context)
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
