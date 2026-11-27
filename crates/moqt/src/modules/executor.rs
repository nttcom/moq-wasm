use std::{future::Future, pin::Pin};

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(target_arch = "wasm32")]
pub use browser::{JoinHandle, SpawnWrapper, now, set_spawn_wrapper, sleep, spawn, system_time};
#[cfg(target_arch = "wasm32")]
pub(crate) use browser::{timeout, try_spawn, yield_now};
#[cfg(not(target_arch = "wasm32"))]
pub use native::{JoinHandle, SpawnWrapper, now, set_spawn_wrapper, sleep, spawn, system_time};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::{timeout, try_spawn, yield_now};

#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + ?Sized> MaybeSend for T {}
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}

#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSync: Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Sync + ?Sized> MaybeSync for T {}
#[cfg(target_arch = "wasm32")]
pub trait MaybeSync {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSync for T {}

/// Owns a spawned task and aborts it when dropped; a bare `JoinHandle`
/// detaches its task instead.
pub struct AbortOnDrop(JoinHandle);

impl JoinHandle {
    /// Ties the task's lifetime to the returned guard.
    pub fn abort_on_drop(self) -> AbortOnDrop {
        AbortOnDrop(self)
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
#[cfg(target_arch = "wasm32")]
pub(crate) type BoxFuture<T> = Pin<Box<dyn Future<Output = T>>>;
