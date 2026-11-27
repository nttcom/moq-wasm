use std::sync::Arc;

use js_sys::Function;
use moqt::executor::{self, AbortOnDrop};
use tokio::sync::Notify;
use wasm_bindgen::JsValue;

/// Calls the page's callback each time a subscriber asks the source for a new
/// group, until dropped.
pub(super) struct KeyframeRequestTask {
    _join_handle: AbortOnDrop,
}

impl KeyframeRequestTask {
    pub(super) fn run(requested: Arc<Notify>, on_keyframe_request: Function) -> Self {
        let join_handle = executor::spawn("media-broadcast-keyframe-requests", async move {
            loop {
                requested.notified().await;
                if let Err(error) = on_keyframe_request.call0(&JsValue::NULL) {
                    tracing::warn!(?error, "keyframe request callback failed");
                }
            }
        })
        .abort_on_drop();
        Self {
            _join_handle: join_handle,
        }
    }
}
