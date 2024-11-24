//! Primary abstraction around operations and some wrappers.
mod definition;
mod wrapper;

pub use self::{
    definition::{Oneshot, Operation},
    wrapper::StashOutput,
};
