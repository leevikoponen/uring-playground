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
use std::{cell::RefCell, future::IntoFuture, io::Result};

use crate::reactor::Reactor;

pub mod batch;
pub mod future;
pub mod operation;
pub mod reactor;

/// Block on the future by polling it concurrently with driving the reactor.
///
/// # Errors
///
/// If [`Reactor::wait_for_progress`] results in an error.
pub fn block_on<T>(reactor: &RefCell<Reactor>, future: impl IntoFuture<Output = T>) -> Result<T> {
    futures_lite::future::block_on(futures_lite::future::or(
        async { Ok(future.await) },
        async {
            loop {
                reactor.borrow_mut().wait_for_progress(None)?;
                futures_lite::future::yield_now().await;
            }
        },
    ))
}
