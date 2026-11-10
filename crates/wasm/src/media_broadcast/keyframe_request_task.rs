use std::sync::Arc;

use js_sys::Function;
use moqt::executor::{self, JoinHandle};
use tokio::sync::Notify;
use wasm_bindgen::JsValue;

/// Calls the page's callback each time a subscriber asks the source for a new
/// group, until dropped.
pub(super) struct KeyframeRequestTask {
    join_handle: JoinHandle,
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
        });
        Self { join_handle }
    }
}

impl Drop for KeyframeRequestTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
