#[derive(Clone, Debug)]
pub(crate) enum LatestInfo {
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
