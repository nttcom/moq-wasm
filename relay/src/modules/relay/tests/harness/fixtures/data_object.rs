use bytes::Bytes;
use moqt::{ObjectStatus, SubgroupId};

use crate::modules::core::data_object::DataObject;

pub(crate) fn ordered_payload(index: usize) -> Bytes {
    Bytes::from(format!("ordered-object-{index}"))
}

pub(crate) fn make_header(group_id: u64) -> DataObject {
    make_header_with(group_id, SubgroupId::Value(0), false)
}

pub(crate) fn make_header_with(
    group_id: u64,
    subgroup_id: SubgroupId,
    has_end_of_group: bool,
) -> DataObject {
    DataObject::SubgroupHeader(moqt::SubgroupHeader::new(
        0,
        group_id,
        subgroup_id,
        128,
        false,
        has_end_of_group,
    ))
}

pub(crate) fn make_payload_object(object_id_delta: u64, payload: Bytes) -> DataObject {
    make_subgroup_object(object_id_delta, moqt::SubgroupObject::new_payload(payload))
}

pub(crate) fn make_status_object(object_id_delta: u64, status: ObjectStatus) -> DataObject {
    make_subgroup_object(
        object_id_delta,
        moqt::SubgroupObject::new_status(u8::from(status) as u64),
    )
}

fn make_subgroup_object(object_id_delta: u64, subgroup_object: moqt::SubgroupObject) -> DataObject {
    let message_type =
        moqt::SubgroupHeader::new(0, 0, SubgroupId::Value(0), 128, false, false).message_type;
    DataObject::SubgroupObject(moqt::SubgroupObjectField {
        message_type,
        object_id_delta,
        extension_headers: moqt::ExtensionHeaders::default(),
        subgroup_object,
    })
}
