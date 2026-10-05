use std::{pin::Pin, task::Poll};

use crate::{Handshake, TransportProtocol, modules::executor::BoxFuture};

pub struct Accepting<T: TransportProtocol> {
    pub(crate) inner: BoxFuture<anyhow::Result<Handshake<T>>>,
}

impl<T: TransportProtocol> Future for Accepting<T> {
    type Output = anyhow::Result<Handshake<T>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}
