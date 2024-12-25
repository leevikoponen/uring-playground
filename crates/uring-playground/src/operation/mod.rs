//! Strongly typed operation definitions that handle safety requirements.
//!
//! # Missing full `io_safety`
//!
//! It might seem odd that I've ended up mostly operating with raw file
//! descriptor types, this is required due to the fact that I can't accept
//! making the use of linked operations overly restrictive.
//!
//! Unfortunately some *terminal* operations like `close` would fundamentally
//! require ownership of the resource handle, but might still want to be batched
//! together as the last with other operations dealing with the same file
//! descriptor, causing a classic self referential situation that can't
//! ergonomically be worked around.
//!
//! This feels like a quite commonly useful situation, so we'll just have to
//! live with the fact that there is a bit of unfortunate misuse potential,
//! but at least it's a runtime error instead of anything worse.
//!
//! # Multishot and `IOSQE_IO_LINK`
//!
//! While it's not possible to have something happen after a final multishot
//! completion, as one could naively look for, it could still be relatively
//! useful to tie a linked operation to the first completion as the interface
//! actually works.
//!
//! Unfortunately, in practice this is pretty hard to represent and I've just
//! given up, instead treating linked operations as batches that complete in
//! their totality, although with the potential of some being interrupted.
mod batch;
mod fs;
mod general;
mod io;
mod net;
mod other;
mod sync;
mod util;

use std::{
    any::Any,
    io::Result,
    pin::Pin,
    task::{Context, Poll},
};

use io_uring::{cqueue, squeue};

pub use self::{
    batch::{Chain2, Chain3, Chain4, Chain5, Chain6},
    fs::{Fsync, Link, MkDir, Open, Rename, Stat, Symlink, Unlink},
    general::{Cancel, Close, FixedFdInstall, PollAdd, PollRemove},
    io::{Read, ReadMulti, Splice, Write},
    net::{Accept, AcceptMulti, Bind, Connect, Listen, Shutdown, Socket},
    other::{LinkedTimeout, MsgRingSendFd, Nop, Raw},
    sync::{FutexWait, FutexWake},
    util::{MapOutput, StashOutput},
};
use crate::driver::{Linked, Multishot, Oneshot, OperationId, Reactor};

/// Abstract representation of a singular operation.
///
/// # Safety
///
/// Implementations must ensure that parameters like mutable buffers that are
/// used after the kernel consuming the SQE are kept alive for the entire
/// duration of the operation, potentially through yielding them from
/// [`Operation::take_allocations`], but keeping in mind that it's not
/// guaranteed to be called.
///
/// # Notes
///
/// What's considered safe in Rust is a little disappointing for us here, we'd
/// essentially be perfectly happy with memory leaks being so, but not usually
/// being able to depend on [`Drop::drop`] being called (such as with stack
/// variables and [`std::mem::forget`]) would essentially force us to heap
/// allocate all pointer parameters, despite us likely being in a context of a
/// heap allocated future as it isn't feasible to prove within the type system.
///
/// Thankfully we can work around this for the values that just need to be alive
/// when submitting due to the drop guarantee described in [`std::pin`] and the
/// fact that our reactor implementation can buffer the submissions and only
/// give them to the kernel when calling submit, allowing it to easily remove
/// entries with parameters from dropped operations.
#[must_use]
pub unsafe trait Operation: Sized {
    /// What this operation ultimately produces.
    type Output;

    /// Build a submission that represents this operation.
    ///
    /// # Notes
    ///
    /// Reading the safety notice should make it clear that actually using the
    /// result requires one to rely on `IORING_FEAT_SUBMIT_STABLE` as otherwise
    /// the requirements for submitting entries are difficult to meet.
    #[must_use]
    fn build_submission(self: Pin<&mut Self>) -> squeue::Entry;

    /// Process this operation's completion.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the entry corresponds to the submission
    /// from [`Operation::build_submission`].
    #[must_use]
    unsafe fn handle_completion(self: Pin<&mut Self>, entry: cqueue::Entry) -> Self::Output;

    /// Take away allocated values that have to live for the entire duration of
    /// the operation instead of just what's required until the submission has
    /// been made.
    ///
    /// # Notes
    ///
    /// In terms of safety, this is essentially just an inconsequential
    /// addition, however in practice it enables avoiding memory leaks in the
    /// typical case of the operation struct just getting dropped before
    /// completion.
    ///
    /// The purpose of this method is thus to make it possible to keep the
    /// allocations associated with the operation and let them be dropped
    /// properly after receiving the final completion.
    #[must_use]
    fn take_allocations(self: Pin<&mut Self>) -> Option<Box<dyn Any + Send>>;

    /// Convert the operation's output to another type.
    fn map_output<F>(self, conversion: F) -> MapOutput<Self, F> {
        MapOutput::new(self, conversion)
    }

    /// Convert an fallible operation's output to another type.
    ///
    /// # Notes
    ///
    /// This might seem somewhat unnecessary but type inference tends to get a
    /// bit confused in these types of situations and this helper alleviates it.
    fn map_ok<A, B>(self, mut conversion: impl FnMut(A) -> B) -> impl Operation<Output = Result<B>>
    where
        Self: Operation<Output = Result<A>>,
    {
        self.map_output(move |result: Result<A>| result.map(&mut conversion))
    }

    /// Build a linked operation.
    fn link_with<O: Operation>(self, second: O) -> Chain2<Self, O> {
        Chain2::new(self, second)
    }

    /// Connect the operation with a linked timeout.
    fn with_timeout(self, timeout: impl Into<LinkedTimeout>) -> impl Batch<Output = Self::Output> {
        self.link_with(timeout.into())
            .map_output(|(operation, _)| operation)
    }

    /// Create an oneshot operation future.
    fn build_oneshot(self, reactor: &Reactor) -> Oneshot<'_, Self> {
        Oneshot::new(reactor, self)
    }

    /// Create a multishot operation stream.
    fn build_multishot(self, reactor: &Reactor) -> Multishot<'_, Self> {
        Multishot::new(reactor, self)
    }
}

/// Abstract representation of multiple oneshot operations linked together.
///
/// # Notes
///
/// The behaviour is unlikely to make any sense if you utilize multishot
/// operations, as this interface is built to assume only a single completion.
///
/// # Safety
///
/// The same requirements as for [`Operation`] apply.
#[must_use]
pub unsafe trait Batch: Sized {
    /// Handle to store information about submitted entries, allowing to work
    /// with more than just one operation.
    type Handle: Copy + Unpin;

    /// What this set of operations ultimately produces.
    type Output;

    /// Extract the first operation handle.
    ///
    /// # Notes
    ///
    /// Seems a bit weird but since batches finish at the same time,
    /// cancellation can and should be initiated using the first entry.
    fn first_operation(handle: Self::Handle) -> OperationId;

    /// Submit entries onto the specified reactor.
    #[must_use]
    fn submit_entries(
        self: Pin<&mut Self>,
        reactor: &Reactor,
        context: Option<&Context>,
    ) -> Self::Handle;

    /// Poll for progress on the operations.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the handle originates from calling
    /// [`Batch::submit_entries`].
    unsafe fn poll_progress(
        self: Pin<&mut Self>,
        handle: Self::Handle,
        reactor: &Reactor,
        context: &Context,
    ) -> Poll<Self::Output>;

    /// Mark operations as ignored as a cancellation step in case of drop.
    fn drop_operations(self: Pin<&mut Self>, handle: Self::Handle, reactor: &Reactor);

    /// Convert the batch's output to another type.
    fn map_output<F>(self, conversion: F) -> MapOutput<Self, F> {
        MapOutput::new(self, conversion)
    }

    /// Create a future for a batch of operations.
    fn build_submission(self, reactor: &Reactor) -> Linked<'_, Self> {
        Linked::new(reactor, self)
    }
}
