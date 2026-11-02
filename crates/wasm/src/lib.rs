#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod loc;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod log_record;
#[cfg(target_arch = "wasm32")]
mod log_sink;
#[cfg(target_arch = "wasm32")]
mod media_broadcast;
mod mp4;
#[cfg(target_arch = "wasm32")]
mod mp4_broadcast;
mod msf_catalog;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod request_rejection;
#[cfg(target_arch = "wasm32")]
mod session;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod session_routes;

#[cfg(target_arch = "wasm32")]
pub use log_sink::set_log_sink;
#[cfg(target_arch = "wasm32")]
pub use mp4_broadcast::{Mp4Broadcast, PublishedStream};
pub use msf_catalog::*;

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
