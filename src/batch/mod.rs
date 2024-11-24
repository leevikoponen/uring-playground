//! Abstractions for linking operations together.
mod definition;
mod extension;
mod link;
mod single;

pub use self::{
    definition::Batch,
    extension::BatchExt,
    link::{Link2, Link3, Link4, Link5},
    single::Single,
};
