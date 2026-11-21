use broadcast::DemandChanges;
use js_sys::Function;
use moqt::executor::{self, JoinHandle};
use wasm_bindgen::JsValue;

/// Calls the page's callback with each `TrackDemand` of the source, until
/// dropped.
pub(super) struct DemandTask {
    join_handle: JoinHandle,
}

impl DemandTask {
    pub(super) fn run(demand_changes: DemandChanges, on_demand_change: Function) -> Self {
        let join_handle = executor::spawn("media-broadcast-demand", async move {
            while let Some(demand) = demand_changes.next().await {
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
