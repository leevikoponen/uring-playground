//! Primary abstraction around operations and some wrappers.
mod definition;
mod extension;
mod general;
mod wrapper;

pub use self::{
    definition::{Oneshot, Operation},
    extension::OperationExt,
    general::Nop,
    wrapper::StashOutput,
};
