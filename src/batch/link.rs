use std::task::{Context, Poll};

use io_uring::squeue::Flags;

use crate::{
    batch::Batch,
    operation::{Oneshot, Operation as _, StashOutput},
    reactor::{OperationId, Reactor},
};

/// Helper macro to consume an identifier token but actually use something else.
macro_rules! replace_ident {
    ($_:ident, $with:ident) => {
        $with
    };
}

/// Helper macro to define wrapper structs for linking [`Oneshot`] operations.
macro_rules! define_link_structs {
    ($($(#[$attribute:meta])* $struct:ident { $($field:ident: $kind:ident),* })*) => {
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
    #[must_use]
    Link2 { first: A, second: B }
    /// Wrapper to link three operations together.
    #[must_use]
    Link3 { first: A, second: B, third: C }
    /// Wrapper to link four operations together.
    #[must_use]
    Link4 { first: A, second: B, third: C, fourth: D }
    /// Wrapper to link five operations together.
    #[must_use]
    Link5 { first: A, second: B, third: C, fourth: D, fifth: E }
}

macro_rules! impl_link_more {
    ($($old_struct:ident { $($old_field:ident: $old_type:ident),* } => $new_struct:ident { ..$new_field:ident: $new_type:ident })*) => {
        $(
            impl<$($old_type: Oneshot),*> $old_struct<$($old_type),*> {
                /// Add another linked operation.
                pub fn link_more<$new_type: Oneshot>(self, $new_field: $new_type) -> $new_struct<$($old_type,)* $new_type> {
                    $new_struct {
                        $($old_field: self.$old_field,)*
                        $new_field: StashOutput::new($new_field),
                    }
                }
            }
        )*
    };
}

impl_link_more! {
    Link2 { first: A, second: B } => Link3 { ..third: C }
    Link3 { first: A, second: B, third: C } => Link4 { ..fourth: D }
    Link4 { first: A, second: B, third: C, fourth: D } => Link5 { ..fifth: E }
}
