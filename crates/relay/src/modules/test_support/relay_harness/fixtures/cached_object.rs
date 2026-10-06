use bytes::Bytes;
use moqt::{ExtensionHeaders, ObjectStatus};
use tokio::time::Instant;

use super::location;
use crate::modules::data_plane::{
    cache::subgroup_key::SubgroupKey,
    cache::{
        cached_object::{CachedObject, ForwardingPreference, SubgroupHeaderFields},
        track_cache::{FetchCursor, FetchInterrupted, OpenSubgroupGuard, TrackCache},
    },
};

pub(crate) const FIXTURE_PRIORITY: u8 = 128;

pub(crate) fn subgroup_header_fields(group_id: u64, subgroup_id: u64) -> SubgroupHeaderFields {
    SubgroupHeaderFields {
        group_id,
        subgroup_id,
        publisher_priority: FIXTURE_PRIORITY,
    }
}

pub(crate) fn stream_key(group_id: u64) -> SubgroupKey {
    subgroup_header_fields(group_id, 0).key()
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
        location: location(group_id, object_id),
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
pub(crate) fn open_group<'a>(
    cache: &'a TrackCache,
    group_id: u64,
    object_ids: &[u64],
) -> OpenSubgroupGuard<'a> {
    let open = cache.open_subgroup(stream_key(group_id));
    for &object_id in object_ids {
        let _ = open.insert(stream_object(group_id, object_id));
    }
    open
}

pub(crate) fn insert_closed_group(cache: &TrackCache, group_id: u64, object_ids: &[u64]) {
    open_group(cache, group_id, object_ids).finish();
}

pub(crate) fn insert_aborted_group(cache: &TrackCache, group_id: u64, object_ids: &[u64]) {
    drop(open_group(cache, group_id, object_ids));
}

pub(crate) async fn fetch_all(
    cache: &TrackCache,
    start: moqt::Location,
    end: moqt::Location,
    group_order: moqt::GroupOrder,
) -> Result<Vec<moqt::FetchObjectField>, FetchInterrupted> {
    let mut cursor = FetchCursor::new(cache, start, end, group_order);
    let mut objects = Vec::new();
    while let Some(object) = cursor.next().await? {
        objects.push(object.to_fetch_object_field());
    }
    Ok(objects)
}
