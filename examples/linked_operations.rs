//! Example showcasing both basic usage and how we have the ability to submit
//! multiple operations that are linked together.
use std::{cell::RefCell, io::Result};

use uring_playground::{
    batch::BatchExt,
    operation::{Nop, OperationExt},
    reactor::Reactor,
};

fn main() -> Result<()> {
    let reactor = Reactor::new(64).map(RefCell::new)?;
    uring_playground::block_on(&reactor, async {
        Nop::new()
            .link_with(Nop::new())
            .build_submission(&reactor)
            .await;
    })
}
