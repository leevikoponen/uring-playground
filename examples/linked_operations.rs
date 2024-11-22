//! Example showcasing both basic usage and how we have the ability to submit
//! multiple operations that are linked together.
use std::{
    any::Any,
    cell::RefCell,
    io::{Error, Result},
};

use futures_lite::future;
use io_uring::{cqueue, opcode, squeue};
use uring_playground::{
    batch::Link2,
    future::SubmitAndWait,
    operation::{Oneshot, Operation},
    reactor::Reactor,
};

struct Nop;

unsafe impl Operation for Nop {
    type Output = Result<()>;

    fn build_submission(&mut self) -> squeue::Entry {
        opcode::Nop::new().build()
    }

    unsafe fn handle_completion(&mut self, entry: cqueue::Entry) -> Self::Output {
        if entry.result().is_negative() {
            return Err(Error::from_raw_os_error(-entry.result()));
        }

        Ok(())
    }

    fn take_required_allocations(&mut self) -> Option<Box<dyn Any>> {
        None
    }
}

unsafe impl Oneshot for Nop {}

fn main() {
    let reactor = Reactor::new(64).map(RefCell::new).unwrap();
    future::block_on(future::or(
        async {
            let (first, second) = SubmitAndWait::new(&reactor, Link2::new(Nop, Nop)).await;

            first.and(second).unwrap();
        },
        async {
            loop {
                reactor.borrow_mut().wait_for_progress(None).unwrap();
                future::yield_now().await;
            }
        },
    ));
}
