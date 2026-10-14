use bytes::Bytes;
use moqt::object::{ObjectStatus, SubgroupStart};

use crate::modules::{
    session::data_object::DataObject,
    test_support::relay_harness::fixtures::cached_object::{
        FIXTURE_PRIORITY, status_object, stream_object_in_subgroup, stream_object_with_payload,
    },
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

pub(crate) fn make_header(group_id: u64) -> DataObject {
    make_header_with(group_id, 0, false)
}

pub(crate) fn make_header_with(group_id: u64, subgroup_id: u64, ends_group: bool) -> DataObject {
    DataObject::SubgroupHeader(subgroup_start(group_id, subgroup_id, ends_group))
}

/// An object of subgroup 0 of `group_id`.
pub(crate) fn make_payload_object(group_id: u64, object_id: u64, payload: Bytes) -> DataObject {
    DataObject::SubgroupObject(
        stream_object_with_payload(group_id, object_id, payload)
            .object
            .clone(),
    )
}

pub(crate) fn make_object_in_subgroup(
    group_id: u64,
    subgroup_id: u64,
    object_id: u64,
) -> DataObject {
    DataObject::SubgroupObject(
        stream_object_in_subgroup(group_id, subgroup_id, object_id)
            .object
            .clone(),
    )
}

pub(crate) fn make_status_object(
    group_id: u64,
    object_id: u64,
    status: ObjectStatus,
) -> DataObject {
    DataObject::SubgroupObject(status_object(group_id, object_id, status).object.clone())
}
