//! Minimal networking utilities.

use std::{cell::RefCell, io::Result, net::SocketAddr, ops::ControlFlow, task::Poll};

use futures_concurrency::future::FutureGroup;
use futures_lite::{future, stream::StreamExt as _};
use nix::sys::socket::{AddressFamily, Backlog, SockFlag, SockProtocol, SockType};

use crate::{
    driver::{FileIndex, UringFd},
    operation::{AcceptMulti, Bind, Listen, Operation as _, Socket},
    rt::Context,
};

/// Serve incoming connections using the given handler function.
///
/// # Errors
///
/// If either setting up the socket or accepting and handling requests fails.
pub async fn serve_tcp(
    context: impl Context,
    address: SocketAddr,
    handler: impl AsyncFn(FileIndex) -> ControlFlow<Result<()>>,
) -> Result<()> {
    let listener = context
        .submit_oneshot(
            Socket::new(
                match address {
                    SocketAddr::V4(_) => AddressFamily::Inet,
                    SocketAddr::V6(_) => AddressFamily::Inet6,
                },
                SockType::Stream,
                SockProtocol::Tcp,
                SockFlag::SOCK_NONBLOCK | SockFlag::SOCK_CLOEXEC,
            )
            .use_fixed(),
        )
        .await?;

    let (first, second) = context
        .submit_linked(
            Bind::new(UringFd::Fixed(listener), address.into())
                .link_with(Listen::new(UringFd::Fixed(listener), Backlog::MAXCONN)),
        )
        .await;

    first.and(second)?;

    let connections = RefCell::new(FutureGroup::new());
    let mut incoming = std::pin::pin!(context.submit_multishot(AcceptMulti::new(
        listener,
        SockFlag::SOCK_NONBLOCK | SockFlag::SOCK_CLOEXEC
    )));

    future::poll_fn(|context| {
        loop {
            while let Poll::Ready(Some(output)) = connections.borrow_mut().poll_next(context) {
                if let ControlFlow::Break(result) = output {
                    return Poll::Ready(result);
                }
            }

            match std::task::ready!(incoming.as_mut().poll_next(context)) {
                Some(Ok(socket)) => _ = connections.borrow_mut().insert(handler(socket)),
                Some(Err(error)) => return Poll::Ready(Err(error)),
                None if connections.borrow().is_empty() => return Poll::Ready(Ok(())),
                None => (),
            }
        }
    })
    .await
}
