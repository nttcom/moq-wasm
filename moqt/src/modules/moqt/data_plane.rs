#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod codec;
pub(crate) mod datagram;
pub(crate) mod object;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod stream;
