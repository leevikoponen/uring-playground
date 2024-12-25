//! The meat of actually submitting operations and waiting for them to complete.
//!
//! # Tracked mutable state
//!
//! The reactor handles state associated with operations, which is used to store
//! wakers used to notify caller tasks of progress and storing the completions
//! that the caller task can then receive when finally executed, i.e. ultimately
//! connects the `io_uring` interface to the more poll oriented async world.
//!
//! # The odd [`Send`] requirement
//!
//! There are a some situations like with ring messages where some initial setup
//! must be done while being able to refer to each other before spawning actual
//! worker threads, which requires passing the instances over after construction
//! on the initial thread.
//!
//! It could be worked around by exposing some intermediary half prepared
//! reactor type that doesn't contain these troublesome fields, but I don't
//! really see it as worth solving when this quirk only ends up effecting the
//! mutable buffer allocations stashed away on operation drop, which will
//! realistically always fulfill said requirement.
mod buffer;
mod future;
mod reactor;
mod types;

pub use self::{
    buffer::{BufferEntry, BufferRegistration, BufferRing},
    future::{Cancellable, Linked, Multishot, Oneshot},
    reactor::{OperationId, Reactor},
    types::{Endpoint, FileIndex, Location, StableBuffer, UringFd},
};
