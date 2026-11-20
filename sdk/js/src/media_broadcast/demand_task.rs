use js_sys::Function;
use moqt::{
    executor::{self, JoinHandle},
    publish::TrackDemand,
};
use tokio::sync::mpsc;
use wasm_bindgen::JsValue;

/// Calls the page's callback with each `TrackDemand` of the source, until
/// dropped.
pub(super) struct DemandTask {
    join_handle: JoinHandle,
}

impl DemandTask {
    pub(super) fn run(
        mut demand_receiver: mpsc::UnboundedReceiver<TrackDemand>,
        on_demand_change: Function,
    ) -> Self {
        let join_handle = executor::spawn("media-broadcast-demand", async move {
            while let Some(demand) = demand_receiver.recv().await {
                if let Err(error) = on_demand_change.call2(
                    &JsValue::NULL,
                    &JsValue::from_str(&demand.track_name),
                    &JsValue::from_bool(demand.wanted),
                ) {
                    tracing::warn!(?error, "demand callback failed");
                }
            }
        });
        Self { join_handle }
    }
}

impl Drop for DemandTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
