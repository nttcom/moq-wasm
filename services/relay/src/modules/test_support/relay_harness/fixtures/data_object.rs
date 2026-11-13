use bytes::Bytes;
use moqt::object::{ObjectStatus, SubgroupPart, SubgroupStart};

use crate::modules::test_support::relay_harness::fixtures::cached_object::{
    FIXTURE_PRIORITY, arriving_object, status_object, stream_object_in_subgroup,
    stream_object_with_payload,
};

pub(crate) fn ordered_payload(index: usize) -> Bytes {
    Bytes::from(format!("ordered-object-{index}"))
}

pub(crate) fn subgroup_start(group_id: u64, subgroup_id: u64, ends_group: bool) -> SubgroupStart {
    SubgroupStart {
        group_id,
        subgroup_id,
        publisher_priority: FIXTURE_PRIORITY,
        ends_group,
    }
}

pub(crate) fn make_header(group_id: u64) -> SubgroupPart {
    make_header_with(group_id, 0, false)
}

pub(crate) fn make_header_with(group_id: u64, subgroup_id: u64, ends_group: bool) -> SubgroupPart {
    SubgroupPart::Header(subgroup_start(group_id, subgroup_id, ends_group))
}

/// An object of subgroup 0 of `group_id`.
pub(crate) fn make_payload_object(group_id: u64, object_id: u64, payload: Bytes) -> SubgroupPart {
    SubgroupPart::Object(
        stream_object_with_payload(group_id, object_id, payload)
            .object
            .clone(),
    )
}

pub(crate) fn make_object_in_subgroup(
    group_id: u64,
    subgroup_id: u64,
    object_id: u64,
) -> SubgroupPart {
    SubgroupPart::Object(
        stream_object_in_subgroup(group_id, subgroup_id, object_id)
            .object
            .clone(),
    )
}

pub(crate) fn make_status_object(
    group_id: u64,
    object_id: u64,
    status: ObjectStatus,
) -> SubgroupPart {
    SubgroupPart::Object(status_object(group_id, object_id, status).object.clone())
}

/// The start of object `object_id` of subgroup 0 whose `payload_len` bytes
/// follow as `make_payload_piece`s.
pub(crate) fn make_object_start(group_id: u64, object_id: u64, payload_len: usize) -> SubgroupPart {
    SubgroupPart::ObjectStart {
        object: arriving_object(group_id, object_id),
        payload_len,
    }
}

pub(crate) fn make_payload_piece(data: &'static [u8], last: bool) -> SubgroupPart {
    SubgroupPart::Payload {
        data: Bytes::from_static(data),
        last,
    }
}
