#[derive(Debug, Clone)]
pub(crate) enum DataObject {
    SubgroupHeader(moqt::SubgroupHeader),
    SubgroupObject(moqt::SubgroupObjectField),
    ObjectDatagram(moqt::ObjectDatagram),
}

impl DataObject {
    pub(crate) fn group_id(&self) -> Option<u64> {
        match &self {
            Self::SubgroupHeader(header) => Some(header.group_id),
            Self::SubgroupObject(_) => None,
            Self::ObjectDatagram(datagram) => Some(datagram.group_id),
        }
    }

    pub(crate) fn object_id(&self) -> Option<u64> {
        match &self {
            Self::SubgroupHeader(_) => None,
            Self::SubgroupObject(object) => Some(object.object_id_delta),
            Self::ObjectDatagram(datagram) => datagram.field.object_id(),
        }
    }
}
