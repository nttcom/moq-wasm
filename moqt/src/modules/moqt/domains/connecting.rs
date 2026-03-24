use std::{pin::Pin, task::Poll};

use crate::{Session, TransportProtocol};

pub struct Connecting<T: TransportProtocol> {
    pub(crate) inner: Pin<Box<dyn Future<Output = anyhow::Result<Session<T>>> + Send>>,
}

impl<T: TransportProtocol> Future for Connecting<T> {
    type Output = anyhow::Result<Session<T>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}
