use std::{pin::Pin, task::Poll};

use crate::{Session, modules::executor::BoxFuture};

pub struct Connecting {
    pub(crate) inner: BoxFuture<anyhow::Result<Session>>,
}

impl Future for Connecting {
    type Output = anyhow::Result<Session>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}
