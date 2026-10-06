use std::{pin::Pin, task::Poll};

use crate::{Handshake, modules::executor::BoxFuture};

pub struct Accepting {
    pub(crate) inner: BoxFuture<anyhow::Result<Handshake>>,
}

impl Future for Accepting {
    type Output = anyhow::Result<Handshake>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}
