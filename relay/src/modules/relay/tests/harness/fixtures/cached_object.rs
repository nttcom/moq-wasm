use bytes::Bytes;
use moqt::{ExtensionHeaders, ObjectStatus};
use tokio::time::Instant;

use crate::modules::relay::{
    cache::{
        cached_object::{CachedObject, ForwardingPreference, SubgroupStream},
        track_cache::{LiveSubgroup, TrackCache},
    },
    types::SubgroupKey,
};

pub(crate) const FIXTURE_PRIORITY: u8 = 128;

pub(crate) fn subgroup_stream(group_id: u64, subgroup_id: u64) -> SubgroupStream {
    SubgroupStream {
        group_id,
        subgroup_id,
        publisher_priority: FIXTURE_PRIORITY,
    }
}

pub(crate) fn stream_key(group_id: u64) -> SubgroupKey {
    subgroup_stream(group_id, 0).key()
}

pub(crate) fn stream_object(group_id: u64, object_id: u64) -> CachedObject {
    stream_object_with_payload(group_id, object_id, Bytes::from_static(b"payload"))
}

pub(crate) fn stream_object_in_subgroup(
    group_id: u64,
    subgroup_id: u64,
    object_id: u64,
) -> CachedObject {
    CachedObject {
        forwarding: ForwardingPreference::Subgroup { subgroup_id },
        ..stream_object(group_id, object_id)
    }
}

pub(crate) fn stream_object_with_payload(
    group_id: u64,
    object_id: u64,
    payload: Bytes,
) -> CachedObject {
    CachedObject {
        location: moqt::Location {
            group_id,
            object_id,
        },
        forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
        publisher_priority: FIXTURE_PRIORITY,
        status: ObjectStatus::Normal,
        extension_headers: ExtensionHeaders::default(),
        payload,
        received_at: Instant::now(),
    }
}

pub(crate) fn status_object(group_id: u64, object_id: u64, status: ObjectStatus) -> CachedObject {
    CachedObject {
        status,
        payload: Bytes::new(),
        ..stream_object(group_id, object_id)
    }
}

pub(crate) fn datagram_object(group_id: u64, object_id: u64) -> CachedObject {
    CachedObject {
        forwarding: ForwardingPreference::Datagram,
        ..stream_object(group_id, object_id)
    }
}

/// Opens subgroup 0 of `group_id` as live ingest, inserts the objects, and
/// hands back the open subgroup so the caller decides when it closes.
pub(crate) fn open_live_group<'a>(
    cache: &'a TrackCache,
    group_id: u64,
    object_ids: &[u64],
) -> LiveSubgroup<'a> {
    let live = cache.open_live_subgroup(stream_key(group_id));
    for &object_id in object_ids {
        let _ = live.insert(stream_object(group_id, object_id));
    }
    live
}

pub(crate) fn insert_closed_live_group(cache: &TrackCache, group_id: u64, object_ids: &[u64]) {
    drop(open_live_group(cache, group_id, object_ids));
}
