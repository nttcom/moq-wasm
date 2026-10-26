use wasm_bindgen::prelude::*;

/// JavaScript-friendly wrapper for subgroup send state
#[wasm_bindgen]
#[derive(Clone, Debug)]
pub struct SubgroupState {
    #[wasm_bindgen(readonly, js_name = trackAlias)]
    pub track_alias: u64,
    #[wasm_bindgen(readonly, js_name = groupId)]
    pub group_id: u64,
    #[wasm_bindgen(readonly, js_name = subgroupId)]
    pub subgroup_id: u64,
    #[wasm_bindgen(readonly, js_name = objectId)]
    pub object_id: u64,
    #[wasm_bindgen(readonly, js_name = headerSent)]
    pub header_sent: bool,
}

impl SubgroupState {
    pub(crate) fn with_track(track_alias: u64) -> Self {
        Self {
            track_alias,
            group_id: js_sys::Date::now() as u64,
            subgroup_id: 0,
            object_id: 0,
            header_sent: false,
        }
    }

    pub(crate) fn mark_header_sent(&mut self) {
        self.header_sent = true;
    }

    pub(crate) fn increment_object_id(&mut self) {
        self.object_id = self.object_id.saturating_add(1);
    }
}
