#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) enum LatestInfo {
    StreamOpened {
        track_key: u128,
        group_id: u64,
    },
    DatagramOpened {
        track_key: u128,
        group_id: u64,
    },
    LatestObject {
        track_key: u128,
        group_id: u64,
        offset: u64,
    },
    EndOfGroup {
        track_key: u128,
        group_id: u64,
    },
}
