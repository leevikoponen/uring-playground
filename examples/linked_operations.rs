//! Example showcasing both basic usage and how we have the ability to submit
//! multiple operations that are linked together.
use std::cell::RefCell;

use futures_lite::future;
use uring_playground::{batch::Link2, future::SubmitAndWait, operation::Nop, reactor::Reactor};

fn main() {
    let reactor = Reactor::new(64).map(RefCell::new).unwrap();
    future::block_on(future::or(
        async {
            SubmitAndWait::new(&reactor, Link2::new(Nop::new(), Nop::new())).await;
        },
        async {
            loop {
                reactor.borrow_mut().wait_for_progress(None).unwrap();
                future::yield_now().await;
            }
        },
    ));
}
