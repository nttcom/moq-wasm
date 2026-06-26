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
            // Datagram types 0x04-0x07 omit the object_id on the wire; it is implicit
            // (previous + 1, or 0 for the first object in the group).
            Self::ObjectDatagram(datagram) => Some(
                datagram
                    .field
                    .object_id()
                    .unwrap_or_else(|| prev_object_id.map_or(0, |prev| prev + 1)),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use moqt::{DatagramField, ObjectDatagram};

    fn datagram_with_id(object_id: u64) -> DataObject {
        DataObject::ObjectDatagram(ObjectDatagram::new(
            0,
            0,
            DatagramField::Payload0x00 {
                object_id,
                publisher_priority: 0,
                payload: Bytes::new(),
            },
        ))
    }

    fn datagram_without_id() -> DataObject {
        // Type 0x06 omits the object_id on the wire (implicit id).
        DataObject::ObjectDatagram(ObjectDatagram::new(
            0,
            0,
            DatagramField::Payload0x06WithEndOfGroup {
                publisher_priority: 0,
                payload: Bytes::new(),
            },
        ))
    }

    #[test]
    fn datagram_with_explicit_id_uses_that_id() {
        // Arrange: a datagram carrying object_id 7 on the wire
        let datagram = datagram_with_id(7);
        // Act / Assert: the explicit id wins, ignoring prev
        assert_eq!(datagram.resolve_absolute_object_id(Some(3)), Some(7));
    }

    #[test]
    fn datagram_without_id_uses_prev_plus_one() {
        // Arrange: a datagram with an implicit object_id, prev resolved to 3
        let datagram = datagram_without_id();
        // Act / Assert: implicit id is prev + 1
        assert_eq!(datagram.resolve_absolute_object_id(Some(3)), Some(4));
    }

    #[test]
    fn datagram_without_id_starts_at_zero() {
        // Arrange: the first datagram in a group has no prev
        let datagram = datagram_without_id();
        // Act / Assert: implicit id starts at 0
        assert_eq!(datagram.resolve_absolute_object_id(None), Some(0));
    }
}
