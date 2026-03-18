use wasm_bindgen::prelude::*;

/// JavaScript-friendly wrapper for subgroup send state
#[wasm_bindgen]
#[derive(Clone)]
pub struct SubgroupState {
    track_alias: u64,
    group_id: u64,
    subgroup_id: u64,
    object_id: u64,
    header_sent: bool,
}

#[wasm_bindgen]
impl SubgroupState {
    #[wasm_bindgen(getter, js_name = trackAlias)]
    pub fn track_alias(&self) -> u64 {
        self.track_alias
    }

    #[wasm_bindgen(getter, js_name = groupId)]
    pub fn group_id(&self) -> u64 {
        self.group_id
    }

    #[wasm_bindgen(getter, js_name = subgroupId)]
    pub fn subgroup_id(&self) -> u64 {
        self.subgroup_id
    }

    #[wasm_bindgen(getter, js_name = objectId)]
    pub fn object_id(&self) -> u64 {
        self.object_id
    }

    #[wasm_bindgen(getter, js_name = headerSent)]
    pub fn header_sent(&self) -> bool {
        self.header_sent
    }
}

impl SubgroupState {
    pub(crate) fn with_track(track_alias: u64) -> Self {
        Self {
            track_alias,
            group_id: 0,
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
