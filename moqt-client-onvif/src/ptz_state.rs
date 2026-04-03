use crate::onvif_nodes::PtzNodeInfo;
use crate::ptz_config::PtzRange;

#[derive(Clone, Debug)]
pub struct PtzState {
    pub range: PtzRange,
    pub node: Option<PtzNodeInfo>,
}

impl PtzState {
    pub fn new(range: PtzRange, node: Option<PtzNodeInfo>) -> Self {
        Self { range, node }
    }
}
