use std::{future::Future, time::Duration};

pub(crate) struct JoinHandle {
    inner: tokio::task::JoinHandle<()>,
}

impl JoinHandle {
    pub(crate) fn abort(&self) {
        self.inner.abort();
    }
}

pub(crate) fn spawn<F>(name: &str, future: F) -> JoinHandle
where
    F: Future<Output = ()> + Send + 'static,
{
    try_spawn(name, future).expect("spawn called outside of a tokio runtime")
}

pub(crate) fn try_spawn<F>(name: &str, future: F) -> Option<JoinHandle>
where
    F: Future<Output = ()> + Send + 'static,
{
    let runtime = tokio::runtime::Handle::try_current().ok()?;
    let inner = tokio::task::Builder::new()
        .name(name)
        .spawn_on(future, &runtime)
        .expect("tokio runtime is shutting down");
    Some(JoinHandle { inner })
}

pub(crate) async fn yield_now() {
    tokio::task::yield_now().await
}

pub(crate) async fn timeout<F: Future>(duration: Duration, future: F) -> Result<F::Output, ()> {
    tokio::time::timeout(duration, future).await.map_err(drop)
}
