use std::cell::RefCell;

use crate::{batch::Batch, future::SubmitAndWait, reactor::Reactor};

/// Extension trait for adding helper methods to batches.
pub trait BatchExt {
    /// Create a submission future.
    fn build_submission(self, reactor: &RefCell<Reactor>) -> SubmitAndWait<'_, Self>
    where
        Self: Batch + Sized,
    {
        SubmitAndWait::new(reactor, self)
    }
}

impl<B> BatchExt for B {}
