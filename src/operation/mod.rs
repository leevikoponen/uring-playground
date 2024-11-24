//! Primary abstraction around operations and some wrappers.
mod definition;
mod general;
mod wrapper;

pub use self::{
    definition::{Oneshot, Operation},
    general::Nop,
    wrapper::StashOutput,
};
