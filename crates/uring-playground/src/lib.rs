//! Minimal crate for somewhat ergonomically dealing with `io_uring`.
//!
//! ```
//! use std::io::Result;
//!
//! use nix::{fcntl::OFlag, sys::stat::Mode};
//! use uring_playground::{
//!     driver::{Endpoint, Location, Reactor, UringFd},
//!     operation::{Batch as _, Close, Open, Operation as _, Write},
//! };
//!
//! fn main() -> Result<()> {
//!     let reactor = Reactor::initialize(64, None)?;
//!
//!     reactor.use_inner(|submitter| submitter.register_files_sparse(16))?;
//!
//!     reactor.block_on(None, None, async {
//!         let output = Open::new(Location::new(c"/dev/null"), Mode::empty(), OFlag::O_WRONLY)
//!             .use_fixed()
//!             .build_oneshot(&reactor)
//!             .await?;
//!
//!         let (write, close) = Write::new(Endpoint::new(UringFd::Fixed(output)), b"some data")
//!             .link_with(Close::new(UringFd::Fixed(output)))
//!             .build_submission(&reactor)
//!             .await;
//!
//!         write.and(close)
//!     })?
//! }
//! ```
//!
//! # Abstraction level rationale
//!
//! I'm not convinced about the currently existing crates that are focused on
//! building higher level bindings. I'm sure they all work fine and likely even
//! perform favourably to `epoll` and friends, but they're not really utilizing
//! anything that makes the interface unique.
//!
//! In comparison, I'm strongly focusing on enabling the use of multishot
//! completions, linked operations, fixed file descriptors, buffer pools,
//! passing fixed file descriptors between rings and keeping control of how
//! submissions happen, which mostly don't neatly map to aiming a structure
//! similar the standard library's interface.
//!
//! # Warning about soundness
//!
//! Dealing with `io_uring` is notoriously a bit tricky with the way ownership
//! is shared with the kernel instead of things happening within a syscall
//! context switch. While I have spent a lot of time thinking about my
//! abstractions, I can't prentend that I'm totally certain about my various
//! assumptions being correct.
//!
//! # Less stable additional features
//!
//! There is a limited amount of higher level abstractions that I'm less
//! convinced about exposed under the feature `extra`. These are absolutely just
//! me playing around, but could still potentially be useful to someone.
#[cfg(feature = "extra")]
pub mod compat;
pub mod driver;
#[cfg(feature = "extra")]
pub mod net;
pub mod operation;
#[cfg(feature = "extra")]
pub mod rt;
#[cfg(feature = "extra")]
pub mod sync;
