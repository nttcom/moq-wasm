use std::{pin::Pin, task::Poll};

use crate::{Handshake, TransportProtocol};

pub struct Accepting<T: TransportProtocol> {
    pub(crate) inner: Pin<Box<dyn Future<Output = anyhow::Result<Handshake<T>>> + Send>>,
}

impl<T: TransportProtocol> Future for Accepting<T> {
    type Output = anyhow::Result<Handshake<T>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}
