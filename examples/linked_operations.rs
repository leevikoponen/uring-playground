//! Example showcasing both basic usage and how we have the ability to submit
//! multiple operations that are linked together.
use std::{cell::RefCell, io::Result};

use uring_playground::{
    operation::{Batch as _, Nop, Oneshot as _},
    reactor::Reactor,
};

fn main() -> Result<()> {
    let reactor = Reactor::new(64).map(RefCell::new)?;
    uring_playground::block_on(&reactor, async {
        let (first, second, third) = Nop::new()
            .link_with(Nop::new())
            .link_more(Nop::new())
            .build_submission(&reactor)
            .await;

        first.and(second).and(third)
    })?
}
