use std::{
    future::Future,
    sync::OnceLock,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures::future::BoxFuture;

static START: OnceLock<Instant> = OnceLock::new();

pub type SpawnWrapper = fn(BoxFuture<'static, ()>) -> BoxFuture<'static, ()>;

static SPAWN_WRAPPER: OnceLock<SpawnWrapper> = OnceLock::new();

/// `spawn` passes every task through `wrapper`, calling it where the task is
/// spawned, as the wasm32 executor's `set_spawn_wrapper` does. Only the first
/// call takes effect.
pub fn set_spawn_wrapper(wrapper: SpawnWrapper) {
    let _ = SPAWN_WRAPPER.set(wrapper);
}

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
    let inner = match SPAWN_WRAPPER.get() {
        Some(wrap) => spawn_on(&runtime, name, wrap(Box::pin(future))),
        None => spawn_on(&runtime, name, future),
    };
    Some(JoinHandle { inner })
}

fn spawn_on<F>(
    runtime: &tokio::runtime::Handle,
    name: &str,
    future: F,
) -> tokio::task::JoinHandle<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    // `.cargo/config.toml` sets `--cfg tokio_unstable` only for this workspace;
    // crates depending on moqt build without it.
    #[cfg(tokio_unstable)]
    let inner = tokio::task::Builder::new()
        .name(name)
        .spawn_on(future, runtime)
        .expect("tokio runtime is shutting down");
    #[cfg(not(tokio_unstable))]
    let inner = {
        let _ = name;
        runtime.spawn(future)
    };
    inner
}

pub(crate) async fn yield_now() {
    tokio::task::yield_now().await
}

pub async fn sleep(duration: Duration) {
    tokio::time::sleep(duration).await
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::future::BoxFuture;
    use tokio::sync::oneshot;

    use super::{set_spawn_wrapper, spawn};

    static WRAPPED: AtomicUsize = AtomicUsize::new(0);

    /// The wrapper is process-wide and set once; it only counts, so the other
    /// tests' tasks run as before.
    fn counting(future: BoxFuture<'static, ()>) -> BoxFuture<'static, ()> {
        WRAPPED.fetch_add(1, Ordering::Relaxed);
        future
    }

    #[tokio::test]
    async fn a_spawned_task_passes_through_the_spawn_wrapper() {
        // Arrange
        set_spawn_wrapper(counting);
        let before = WRAPPED.load(Ordering::Relaxed);
        let (ran_sender, ran_receiver) = oneshot::channel();

        // Act
        let _task = spawn("wrapped", async move {
            let _ = ran_sender.send(());
        });

        // Assert
        ran_receiver.await.unwrap();
        assert!(WRAPPED.load(Ordering::Relaxed) > before);
    }
}
