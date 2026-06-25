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

    /// Resolves the absolute object_id of this object within its ingest stream.
    /// `prev_object_id` is the resolved object_id of the previous object on the same
    /// stream (`None` at the start of a subgroup or right after its header).
    pub(crate) fn resolve_absolute_object_id(&self, prev_object_id: Option<u64>) -> Option<u64> {
        match self {
            Self::SubgroupHeader(_) => None,
            Self::SubgroupObject(field) => Some(field.resolve_object_id(prev_object_id)),
            Self::ObjectDatagram(datagram) => datagram.field.object_id(),
        }
    }
}
