use bytes::Bytes;

use crate::modules::core::data_object::DataObject;

pub(crate) fn ordered_payload(index: usize) -> Bytes {
    Bytes::from(format!("ordered-object-{index}"))
}

pub(crate) fn make_header(group_id: u64) -> DataObject {
    DataObject::SubgroupHeader(moqt::SubgroupHeader::new(
        0,
        group_id,
        moqt::SubgroupId::Value(0),
        128,
        false,
        false,
    ))
}

pub(crate) fn make_payload_object(object_id_delta: u64, payload: Bytes) -> DataObject {
    let message_type =
        moqt::SubgroupHeader::new(0, 0, moqt::SubgroupId::Value(0), 128, false, false).message_type;
    DataObject::SubgroupObject(moqt::SubgroupObjectField {
        message_type,
        object_id_delta,
        extension_headers: moqt::ExtensionHeaders::default(),
        subgroup_object: moqt::SubgroupObject::new_payload(payload),
    })
}
