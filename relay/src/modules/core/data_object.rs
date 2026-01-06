pub(crate) struct DataObject {
    pub(crate) moqt_data_object: moqt::DataObject,
}

impl DataObject {
    pub(crate) fn group_id(&self) -> Option<u64> {
        match &self.moqt_data_object {
            moqt::DataObject::SubgroupHeader(header) => Some(header.group_id),
            moqt::DataObject::SubgroupObject(_) => None,
            moqt::DataObject::ObjectDatagram(datagram) => Some(datagram.group_id),
        }
    }

    pub(crate) fn object_id(&self) -> Option<u64> {
        match &self.moqt_data_object {
            moqt::DataObject::SubgroupHeader(_) => None,
            moqt::DataObject::SubgroupObject(object) => Some(object.object_id_delta),
            moqt::DataObject::ObjectDatagram(datagram) => datagram.field.object_id(),
        }
    }
}
