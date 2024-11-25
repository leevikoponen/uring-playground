//! Primary abstraction around operations and some wrappers.
mod definition;
mod future;
mod general;
mod link;
mod synchronization;
mod wrapper;

pub use self::{
    definition::{Batch, Oneshot, Operation},
    future::SubmitAndWait,
    general::{LinkTimeout, Nop},
    link::{Link2, Link3, Link4, Link5},
    synchronization::{FutexWait, FutexWake},
    wrapper::{MapOutput, Single, StashOutput},
};
