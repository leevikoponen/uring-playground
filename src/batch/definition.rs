use std::task::{Context, Poll};

use crate::reactor::Reactor;

/// Abstract representation of multiple [`Oneshot`] operations.
///
/// # Safety
///
/// The same requirements as [`Operation`] apply.
#[must_use = "operations do nothing unless submitted"]
pub unsafe trait Batch: Unpin {
    /// Handle to store information about submitted entries.
    ///
    /// This should allow for working with more than just one entry, such as
    /// with linked operations.
    type Handle: Copy + Unpin;

    /// What this operation ultimately produces.
    type Output: Unpin;

    /// Submit entries onto the specified reactor.
    #[must_use = "submission handle should be stored somewhere"]
    fn submit_entries(&mut self, reactor: &mut Reactor, context: Option<&Context>) -> Self::Handle;

    /// Poll for progress on the operations.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the handle originates from calling
    /// [`Operation::submit_entries`].
    unsafe fn poll_progress(
        &mut self,
        handle: Self::Handle,
        reactor: &mut Reactor,
        context: &Context,
    ) -> Poll<Self::Output>;

    /// Mark operations as ignored as a cancellation step in case of drop.
    fn drop_operations(&mut self, handle: Self::Handle, reactor: &mut Reactor);
}
