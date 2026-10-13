use std::{
    future::Future,
    sync::OnceLock,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static START: OnceLock<Instant> = OnceLock::new();

pub struct JoinHandle {
    inner: tokio::task::JoinHandle<()>,
}

impl JoinHandle {
    pub fn abort(&self) {
        self.inner.abort();
    }
}

/// Panics outside of a tokio runtime.
pub fn spawn<F>(name: &str, future: F) -> JoinHandle
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
    // `.cargo/config.toml` sets `--cfg tokio_unstable` only for this workspace;
    // crates depending on moqt build without it.
    #[cfg(tokio_unstable)]
    let inner = tokio::task::Builder::new()
        .name(name)
        .spawn_on(future, &runtime)
        .expect("tokio runtime is shutting down");
    #[cfg(not(tokio_unstable))]
    let inner = {
        let _ = name;
        runtime.spawn(future)
    };
    Some(JoinHandle { inner })
}

pub(crate) async fn yield_now() {
    tokio::task::yield_now().await
}

pub(crate) async fn timeout<F: Future>(duration: Duration, future: F) -> Result<F::Output, ()> {
    tokio::time::timeout(duration, future).await.map_err(drop)
}

/// Monotonic time since an arbitrary origin; only differences are meaningful.
pub fn now() -> Duration {
    START.get_or_init(Instant::now).elapsed()
}

/// The wall clock as the time since the Unix epoch.
pub fn system_time() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}
