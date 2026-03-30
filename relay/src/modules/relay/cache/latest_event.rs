use crate::modules::types::TrackKey;

#[derive(Clone, Debug)]
pub(crate) enum LatestCacheEvent {
    ObjectAppended {
        track_key: TrackKey,
        group_id: u64,
        index: u64,
    },
    GroupClosed {
        track_key: TrackKey,
        group_id: u64,
    },
}