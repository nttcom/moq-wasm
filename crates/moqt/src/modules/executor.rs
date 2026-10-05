use std::{future::Future, pin::Pin};

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(target_arch = "wasm32")]
pub(crate) use browser::{JoinHandle, spawn, timeout, try_spawn, yield_now};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::{JoinHandle, spawn, timeout, try_spawn, yield_now};

#[derive(Debug)]
pub(crate) struct Elapsed;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + ?Sized> MaybeSend for T {}
#[cfg(target_arch = "wasm32")]
pub(crate) trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) trait MaybeSync: Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Sync + ?Sized> MaybeSync for T {}
#[cfg(target_arch = "wasm32")]
pub(crate) trait MaybeSync {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSync for T {}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
#[cfg(target_arch = "wasm32")]
pub(crate) type BoxFuture<T> = Pin<Box<dyn Future<Output = T>>>;
