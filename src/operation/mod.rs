//! Primary abstraction around operations and some wrappers.
mod definition;
mod general;
mod synchronization;
mod wrapper;

pub use self::{
    definition::{Oneshot, Operation},
    general::Nop,
    synchronization::{FutexWait, FutexWake},
    wrapper::{MapOutput, StashOutput},
};
