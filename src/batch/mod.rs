//! Abstractions for linking operations together.
mod definition;
mod link;
mod single;

pub use self::{
    definition::Batch,
    link::{Link2, Link3, Link4, Link5},
    single::Single,
};
