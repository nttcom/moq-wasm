use crate::{
    DatagramField,
    modules::moqt::data_plane::object::{
        object_datagram::ObjectDatagram,
        subgroup::{SubgroupHeader, SubgroupObjectField},
    },
};

#[derive(Debug, Clone)]
pub enum DataObject {
    SubgroupHeader(SubgroupHeader),
    SubgroupObject(SubgroupObjectField),
    ObjectDatagram(ObjectDatagram),
}

impl DataObject {
    pub fn to_object_datagram(track_alias: u64, group_id: u64, field: DatagramField) -> Self {
        let object = ObjectDatagram::new(track_alias, group_id, field);
        DataObject::ObjectDatagram(object)
    }

    pub fn to_subgroup_header(header: SubgroupHeader) -> Self {
        DataObject::SubgroupHeader(header)
    }

    pub fn to_subgroup_object(field: SubgroupObjectField) -> Self {
        DataObject::SubgroupObject(field)
    }
}
