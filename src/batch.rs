//! Abstractions for linking operations together.
use std::task::{Context, Poll};

use io_uring::squeue::Flags;

use crate::{
    operation::{Oneshot, Operation, StashOutput},
    reactor::{OperationId, Reactor},
};

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
    /// Wrapper to link two operations together.
    Link2 { first: A, second: B }
    /// Wrapper to link three operations together.
    Link3 { first: A, second: B, third: C }
    /// Wrapper to link four operations together.
    Link4 { first: A, second: B, third: C, fourth: D }
    /// Wrapper to link five operations together.
    Link5 { first: A, second: B, third: C, fourth: D, fifth: E }
}
