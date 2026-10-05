#[cfg(target_arch = "wasm32")]
mod client;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod client_state;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod incoming_fetch;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod loc;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod messages;
mod mp4;
mod msf_catalog;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod request_rejection;
mod utils;

#[cfg(target_arch = "wasm32")]
pub use client::MOQTClient;
pub use messages::*;
pub use msf_catalog::*;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
fn main() {
    utils::set_panic_hook();
}

#[cfg(not(target_arch = "wasm32"))]
#[wasm_bindgen]
pub struct MOQTClient;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn js_error(message: impl Into<String>) -> JsValue {
    js_sys::Error::new(&message.into()).into()
}
