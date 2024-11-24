// these are unfortunate hacky macro generated variadic types, thankfully it's
// not important to support particularly many operations linked together as I
// can't think of a sensible use case for that
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
    (
        $(
            $(#[$struct_attribute:meta])*
            $struct_name:ident {
                $(
                    $(#[$field_attribute:meta])*
                    $field_name:ident: $generic_name:ident
                ),*
            }
        )*
    ) => {
        $(
            /// Wrapper to link multiple operations together.
            #[must_use]
            $(#[$struct_attribute])*
            pub struct $struct_name<$($generic_name: Oneshot),*> {
                $(
                    $(#[$field_attribute])*
                    $field_name: StashOutput<$generic_name>
                ),*
            }

            impl<$($generic_name: Oneshot),*> $struct_name<$($generic_name),*> {
                pub const fn new($($field_name: $generic_name),*) -> Self {
                    Self {
                        $(
                            $field_name: StashOutput::new($field_name)
                        ),*
                    }
                }
            }

            // SAFETY: the safety requirements are identical
            unsafe impl<$($generic_name: Oneshot),*> Batch for $struct_name<$($generic_name),*> {
                type Handle = ($(replace_ident!($generic_name, OperationId),)*);
                type Output = ($($generic_name::Output,)*);

                fn submit_entries(
                    &mut self,
                    reactor: &mut Reactor,
                    context: Option<&Context>,
                ) -> Self::Handle {
                    // we're using a fixed size iterator as a wonky way of tracking
                    // if we're at the last entry in order to handle flags correctly
                    let mut entries = [$(self.$field_name.build_submission()),*].into_iter();

                    $(
                        let $field_name = {
                            // pull out the field that corresponds to the current
                            // entry and add the link flag unless it's the last
                            // SAFETY: calling next only as many times as there are entries
                            let entry = unsafe { entries.next().unwrap_unchecked() };
                            let entry = if entries.len() != 0 {
                                entry.flags(Flags::IO_LINK)
                            } else {
                                entry
                            };

                            // SAFETY: operation implementations guarantee safety
                            unsafe { reactor.queue_submission(entry, context) }
                        };
                    )*

                    ($($field_name,)*)
                }

                unsafe fn poll_progress(
                    &mut self,
                    ($($field_name),*): Self::Handle,
                    reactor: &mut Reactor,
                    context: &Context,
                ) -> Poll<Self::Output> {
                    $(
                        // go through and poll any unfinished operations, bailing out unless ready
                        if self.$field_name.not_finished() {
                            let output = reactor.poll_completion($field_name, context).map(|entry| {
                                // SAFETY: caller guarantees that we control the submission
                                unsafe { self.$field_name.handle_completion(entry) }
                            });

                            if output.is_pending() {
                                return Poll::Pending;
                            }
                        }
                    )*

                    Poll::Ready(($(
                        // SAFETY: we've bailed at this point if there isn't output
                        unsafe {
                            self.$field_name.take_output().unwrap_unchecked()
                        }
                    ),*))
                }

                fn drop_operations(
                    &mut self,
                    ($($field_name),*): Self::Handle,
                    reactor: &mut Reactor
                ) {
                    $(
                        reactor.ignore_operation(
                            $field_name,
                            self.$field_name.take_required_allocations()
                        );
                    )*
                }
            }
        )*
    };
}

/// Helper macro to add builder methods for appending another linked operation.
///
/// Somewhat unfortunate that doing this requires a separate macro, but I
/// couldn't think of a way to get the original struct's fields and generic
/// parameters inside an optional pattern which is needed for handling the final
/// wrapper struct that can't implement this method as there's nothing after it.
macro_rules! impl_link_more {
    (
        $(
            $(#[$function_attribute:meta])*
            $original_name:ident {
                $(
                    $field_name:ident: $generic_name:ident
                ),*
            } => $next_name:ident {
                $added_field:ident: $added_generic:ident
            }
        )*
    ) => {
        $(
            impl<$($generic_name: Oneshot),*> $original_name<$($generic_name),*> {
                /// Add another linked operation.
                $(#[$function_attribute])*
                pub fn link_more<$added_generic: Oneshot>(
                    self,
                    $added_field: $added_generic
                ) -> $next_name<$($generic_name,)* $added_generic> {
                    // move values into the new struct with one more field
                    $next_name {
                        $(
                            $field_name: self.$field_name,
                        )*
                        $added_field: StashOutput::new($added_field)
                    }
                }
            }
        )*
    };
}

define_link_structs! {
    Link2 { first: A, second: B }
    Link3 { first: A, second: B, third: C }
    Link4 { first: A, second: B, third: C, fourth: D }
    Link5 { first: A, second: B, third: C, fourth: D, fifth: E }
}

impl_link_more! {
    Link2 { first: A, second: B } => Link3 { third: C }
    Link3 { first: A, second: B, third: C } => Link4 { fourth: D }
    Link4 { first: A, second: B, third: C, fourth: D } => Link5 { fifth: E }
}
