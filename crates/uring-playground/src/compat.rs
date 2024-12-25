//! Compatibility wrappers for integrating with poll based code.
//!
//! # Implementation limitations
//!
//! Currently the approach in this module is to just wait for an oneshot poll
//! operation when encountering `EAGAIN`. While this works, it's not very
//! efficient when you expect IO to happen in many chunks.
//!
//! I have some ideas about building a better abstraction working more like
//! `epoll`'s edge triggered mode internally, but it doesn't feel worth
//! prioritizing at the moment.
use std::{
    io::{ErrorKind, Read, Result, Write},
    os::fd::AsRawFd,
};

use nix::poll::PollFlags;

use crate::{driver::UringFd, operation::PollAdd, rt::Context};

macro_rules! define_poll_wrappers {
    (
        $(
            $(#[$function_attribute:meta])*
            $trait_name:ident::$method_name:ident
            $(($parameter_name:ident: $parameter_type:ty)),* => $return_type:ident
            as $readiness_requirement:ident
        )*
    ) => {
        $(
            $(#[$function_attribute])*
            ///
            /// # Errors
            ///
            /// If either the operation or waiting for readiness fails.
            pub async fn $method_name(
                context: impl Context,
                file: &mut (impl $trait_name + AsRawFd),
                $($parameter_name: $parameter_type),*
            ) -> Result<$return_type> {
                loop {
                    match file.$method_name($($parameter_name),*) {
                        Ok(output) => return Ok(output),
                        Err(error) => match error.kind() {
                            ErrorKind::Interrupted => continue,
                            ErrorKind::WouldBlock => (),
                            _ => return Err(error),
                        },
                    }

                    context.submit_oneshot(PollAdd::new(
                        UringFd::Fd(file.as_raw_fd()),
                        PollFlags::$readiness_requirement
                    ))
                    .await?;
                }
            }
        )*
    };
}

define_poll_wrappers! {
    /// Read from a file descriptor by polling for readiness.
    Read::read(buffer: &mut [u8]) => usize as POLLIN

    /// Write to a file descriptor by polling for readiness.
    Write::write(buffer: &[u8]) => usize as POLLOUT
}
