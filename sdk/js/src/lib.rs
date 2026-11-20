#[cfg(target_arch = "wasm32")]
mod loc;
#[cfg(target_arch = "wasm32")]
mod log_sink;
#[cfg(target_arch = "wasm32")]
mod media_broadcast;
mod mp4;
#[cfg(target_arch = "wasm32")]
mod mp4_broadcast;
mod msf_catalog;
#[cfg(target_arch = "wasm32")]
mod session;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
fn main() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn js_error(message: impl Into<String>) -> JsValue {
    js_sys::Error::new(&message.into()).into()
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn anyhow_error(error: anyhow::Error) -> JsValue {
    js_error(format!("{error:#}"))
}
