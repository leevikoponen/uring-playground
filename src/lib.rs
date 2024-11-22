//! Experimental high-level bindings for interfacing with `io_uring` using
//! hopefully safe `async` code.
//!
//! # Notice
//!
//! This is a purely experimental project, in large parts serving as a learning
//! exercise, I give zero guarantees on the actual soudness of the
//! implementation.
//!
//! It should be fine, but working with raw OS primitives is always somewhat
//! easy to get wrong, especially with how `io_uring` largely centers around the
//! notion of temporarily transferring ownership to the kernel.
pub mod batch;
pub mod future;
pub mod operation;
pub mod reactor;
