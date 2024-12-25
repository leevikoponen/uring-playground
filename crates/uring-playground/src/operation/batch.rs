use std::{
    pin::Pin,
    task::{Context, Poll},
};

use io_uring::squeue::Flags;
use pin_project::pin_project;

use crate::{
    driver::{OperationId, Reactor},
    operation::{Batch, Operation, StashOutput},
};

/// Helper macro to consume a token but actually use something else.
macro_rules! replace_token {
    ($_:tt, $with:tt) => {
        $with
    };
}

/// Helper macro to define wrapper structs for linking operations together.
macro_rules! define_link_structs {
    (
        $(
            $(#[$struct_attribute:meta])*
            $struct_name:ident {
                $(
                    $field_name:ident: $generic_name:ident
                ),*
            }
        )*
    ) => {
        $(
            $(#[$struct_attribute])*
            #[pin_project]
            #[must_use]
            pub struct $struct_name<$($generic_name: Operation),*> {
                $(
                    #[pin]
                    $field_name: StashOutput<$generic_name>,
                )*
            }

            impl<$($generic_name: Operation),*> $struct_name<$($generic_name),*> {
                /// Create a wrapper to connect these operations together.
                pub const fn new($($field_name: $generic_name),*) -> Self {
                    Self {
                        $(
                            $field_name: StashOutput::new($field_name),
                        )*
                    }
                }
            }

            // SAFETY: the safety requirements are identical
            unsafe impl<$($generic_name: Operation),*> Batch for $struct_name<$($generic_name),*> {
                type Handle = ($(replace_token!($generic_name, OperationId)),*);
                type Output = ($($generic_name::Output),*);

                fn first_operation(handle: Self::Handle) -> OperationId {
                    handle.0
                }

                fn submit_entries(
                    self: Pin<&mut Self>,
                    reactor: &Reactor,
                    context: Option<&Context>,
                ) -> Self::Handle {
                    let this = self.project();

                    // we're using a fixed size iterator as a wonky way of tracking
                    // if we're at the last entry in order to handle flags correctly
                    let mut entries = [$(this.$field_name.build_submission()),*].into_iter();

                    $(
                        let $field_name = {
                            // SAFETY: calling next only as many times as there are entries
                            let mut entry = unsafe { entries.next().unwrap_unchecked() };
                            if entries.len() != 0 {
                                entry = entry.flags(Flags::IO_LINK);
                            }

                            // SAFETY: operation implementations guarantee safety
                            unsafe { reactor.enqueue_submission(entry, context) }
                        };
                    )*

                    ($($field_name,)*)
                }

                unsafe fn poll_progress(
                    self: Pin<&mut Self>,
                    ($($field_name),*): Self::Handle,
                    reactor: &Reactor,
                    context: &Context,
                ) -> Poll<Self::Output> {
                    let mut this = self.project();

                    $(
                        if !this.$field_name.has_output() {
                            let output = reactor.poll_completion($field_name, context).map(|entry| {
                                // SAFETY: caller guarantees that we control the submission
                                unsafe { this.$field_name.as_mut().handle_completion(entry) }
                            });

                            if output.is_pending() {
                                return Poll::Pending;
                            }
                        }
                    )*

                    Poll::Ready((
                        $(
                            this.$field_name
                                .take_output()
                                .expect("should have returned early due to incomplete operation")
                        ),*
                    ))
                }

                fn drop_operations(
                    self: Pin<&mut Self>,
                    ($($field_name),*): Self::Handle,
                    reactor: &Reactor
                ) {
                    let this = self.project();

                    $(
                        reactor.ignore_operation(
                            $field_name,
                            this.$field_name.take_allocations()
                        );
                    )*
                }
            }
        )*
    };
}

macro_rules! implement_link_more {
    (
        $(
            $(#[$conversion_attribute:meta])*
            $original_struct:ident {
                $(
                    $original_field:ident: $original_generic:ident
                ),*
            } => $output_struct:ident {
                ...$additional_field:ident: $additional_generic:ident
            }
        )*
    ) => {
        $(
            impl<$($original_generic: Operation),*> $original_struct<$($original_generic),*> {
                $(#[$conversion_attribute])*
                pub fn link_with<$additional_generic: Operation>(
                    self,
                    $additional_field: $additional_generic
                ) -> $output_struct<$($original_generic,)* $additional_generic> {
                    let Self { $($original_field),* } = self;
                    $output_struct {
                        $($original_field,)*
                        $additional_field: StashOutput::new($additional_field),
                    }
                }
            }
        )*
    };
}

define_link_structs! {
    /// Wrapper to link two operations together.
    Chain2 { first: A, second: B }
    /// Wrapper to link three operations together.
    Chain3 { first: A, second: B, third: C }
    /// Wrapper to link four operations together.
    Chain4 { first: A, second: B, third: C, fourth: D }
    /// Wrapper to link five operations together.
    Chain5 { first: A, second: B, third: C, fourth: D, fifth: E }
    /// Wrapper to link six operations together.
    Chain6 { first: A, second: B, third: C, fourth: D, fifth: E, sixth: F }
}

implement_link_more! {
    /// Add a third operation to this batch.
    Chain2 { first: A, second: B } => Chain3 { ...third: C }
    /// Add a fourth operation to this batch.
    Chain3 { first: A, second: B, third: C } => Chain4 { ...fourth: D }
    /// Add a fifth operation to this batch.
    Chain4 { first: A, second: B, third: C, fourth: D } => Chain5 { ...fifth: E }
    /// Add a sixth operation to this batch.
    Chain5 { first: A, second: B, third: C, fourth: D, fifth: E } => Chain6 { ...sixth: F }
}
