use crate::modules::types::TrackKey;

#[derive(Clone, Debug, PartialEq, Eq)]
enum LatestInfo {
    StreamOpened {
        track_key: TrackKey,
        group_id: u64,
        offset: u64,
    },
    DatagramOpened {
        track_key: TrackKey,
    },
    LatestObject {
        track_key: TrackKey,
        group_id: u64,
        offset: u64,
    },
    EndOfGroup {
        track_key: TrackKey,
        group_id: u64,
    },
}
