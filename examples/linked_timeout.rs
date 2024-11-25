//! Example showcasing how `io_uring` allows linked timeouts.
use std::{cell::RefCell, io::Result, os::fd::AsFd as _, time::Duration};

use uring_playground::{
    operation::{Batch as _, LinkTimeout, Oneshot as _, Read},
    reactor::Reactor,
};

fn main() -> Result<()> {
    let reactor = Reactor::new(64).map(RefCell::new)?;
    uring_playground::block_on(&reactor, async {
        let (first, second) = Read::new(std::io::stdin().as_fd(), Vec::with_capacity(512))
            .link_with(LinkTimeout::relative(Duration::from_secs(5)))
            .build_submission(&reactor)
            .await;

        match (first, second) {
            (Ok(buffer), Err(error)) => {
                println!("managed to read {} bytes before timeout", buffer.len());
                assert_eq!(error.raw_os_error(), Some(libc::ECANCELED));
            }
            (Err(first), Err(second)) => {
                println!("read operation timed out");
                assert_eq!(first.raw_os_error(), Some(libc::ECANCELED));
                assert_eq!(second.raw_os_error(), Some(libc::ETIME));
            }
            other => println!("unexpected output: {other:?}"),
        }

        Ok(())
    })?
}
