//! Abstractions for being generic about how operations are driven, such as
//! supporting various forms of cancellation.
use std::time::{Duration, SystemTime};

use futures_lite::Stream;
use local_async::CancellationToken;

use crate::{
    driver::Reactor,
    operation::{Batch, Operation},
};

/// Something that can be used to drive operations.
pub trait Context {
    /// Get access to the reactor.
    fn reactor(&self) -> &Reactor;

    /// Wait for a cancellation to be triggered.
    fn cancelled(&self) -> impl Future<Output = ()> {
        std::future::pending()
    }

    /// Submit an oneshot operation with appropriate cancellation.
    fn submit_oneshot<O: Operation>(&self, operation: O) -> impl Future<Output = O::Output> {
        operation
            .build_oneshot(self.reactor())
            .with_cancellation(self.cancelled())
    }

    /// Submit a multishot operation with appropriate cancellation.
    fn submit_multishot<O: Operation>(&self, operation: O) -> impl Stream<Item = O::Output> {
        operation
            .build_multishot(self.reactor())
            .with_cancellation(self.cancelled())
    }

    /// Submit a linked batch of operations with appropriate cancellation.
    fn submit_linked<B: Batch>(&self, batch: B) -> impl Future<Output = B::Output> {
        batch
            .build_submission(self.reactor())
            .with_cancellation(self.cancelled())
    }
}

impl Context for &Reactor {
    fn reactor(&self) -> &Reactor {
        self
    }

    fn submit_oneshot<O: Operation>(&self, operation: O) -> impl Future<Output = O::Output> {
        operation.build_oneshot(self)
    }

    fn submit_multishot<O: Operation>(&self, operation: O) -> impl Stream<Item = O::Output> {
        operation.build_multishot(self)
    }

    fn submit_linked<B: Batch>(&self, batch: B) -> impl Future<Output = B::Output> {
        batch.build_submission(self)
    }
}

impl Context for (&Reactor, &CancellationToken) {
    fn reactor(&self) -> &Reactor {
        self.0
    }

    fn cancelled(&self) -> impl Future<Output = ()> {
        self.1.wait()
    }
}

impl Context for (&Reactor, Duration) {
    fn reactor(&self) -> &Reactor {
        self.0
    }

    fn submit_oneshot<O: Operation>(&self, operation: O) -> impl Future<Output = O::Output> {
        operation.with_timeout(self.1).build_submission(self.0)
    }
}

impl Context for (&Reactor, SystemTime) {
    fn reactor(&self) -> &Reactor {
        self.0
    }

    fn submit_oneshot<O: Operation>(&self, operation: O) -> impl Future<Output = O::Output> {
        operation.with_timeout(self.1).build_submission(self.0)
    }
}
