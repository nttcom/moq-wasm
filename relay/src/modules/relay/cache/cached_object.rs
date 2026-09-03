use bytes::Bytes;
use moqt::{
    DatagramField, ExtensionHeaders, FetchObject, FetchObjectField, ObjectDatagram,
    ObjectDatagramPayload, ObjectStatus, SubgroupHeaderType, SubgroupObject, SubgroupObjectField,
};
use tokio::time::Instant;

use crate::modules::relay::types::SubgroupKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ForwardingPreference {
    Subgroup { subgroup_id: u64 },
    Datagram,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DuplicateKind {
    Identical,
    Conflict,
}

/// Identity of one upstream subgroup stream, fixed by its SUBGROUP_HEADER.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SubgroupStream {
    pub(crate) group_id: u64,
    pub(crate) subgroup_id: u64,
    pub(crate) publisher_priority: u8,
}

impl SubgroupStream {
    pub(crate) fn key(&self) -> SubgroupKey {
        SubgroupKey::Stream {
            group_id: self.group_id,
            subgroup_id: self.subgroup_id,
        }
    }
}

/// draft-14 §10.2.1 canonical object as stored by the relay cache.
#[derive(Clone, Debug)]
pub(crate) struct CachedObject {
    pub(crate) location: moqt::Location,
    pub(crate) forwarding: ForwardingPreference,
    pub(crate) publisher_priority: u8,
    pub(crate) status: ObjectStatus,
    pub(crate) extension_headers: ExtensionHeaders,
    pub(crate) payload: Bytes,
    pub(crate) received_at: Instant,
}

impl CachedObject {
    pub(crate) fn from_subgroup_object(
        stream: &SubgroupStream,
        object_id: u64,
        field: SubgroupObjectField,
    ) -> anyhow::Result<Self> {
        let (status, payload) = match field.subgroup_object {
            SubgroupObject::Payload { data, .. } => (ObjectStatus::Normal, data),
            SubgroupObject::Status { code, .. } => (Self::status_from_code(code)?, Bytes::new()),
        };
        Ok(Self {
            location: moqt::Location {
                group_id: stream.group_id,
                object_id,
            },
            forwarding: ForwardingPreference::Subgroup {
                subgroup_id: stream.subgroup_id,
            },
            publisher_priority: stream.publisher_priority,
            status,
            extension_headers: field.extension_headers,
            payload,
            received_at: Instant::now(),
        })
    }

    pub(crate) fn end_of_group(stream: &SubgroupStream, object_id: u64) -> Self {
        Self {
            location: moqt::Location {
                group_id: stream.group_id,
                object_id,
            },
            forwarding: ForwardingPreference::Subgroup {
                subgroup_id: stream.subgroup_id,
            },
            publisher_priority: stream.publisher_priority,
            status: ObjectStatus::EndOfGroup,
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::new(),
            received_at: Instant::now(),
        }
    }

    pub(crate) fn from_datagram(object_id: u64, datagram: ObjectDatagram) -> Self {
        let ends_group = matches!(
            datagram.field,
            DatagramField::Payload0x02WithEndOfGroup { .. }
                | DatagramField::Payload0x03WithEndOfGroup { .. }
                | DatagramField::Payload0x06WithEndOfGroup { .. }
                | DatagramField::Payload0x07WithEndOfGroup { .. }
        );
        let (status, payload) = match datagram.field.payload() {
            ObjectDatagramPayload::Payload(payload) if ends_group => {
                (ObjectStatus::EndOfGroup, payload)
            }
            ObjectDatagramPayload::Payload(payload) => (ObjectStatus::Normal, payload),
            ObjectDatagramPayload::Status(status) => (status, Bytes::new()),
        };
        Self {
            location: moqt::Location {
                group_id: datagram.group_id,
                object_id,
            },
            forwarding: ForwardingPreference::Datagram,
            publisher_priority: datagram.field.publisher_priority(),
            status,
            extension_headers: datagram
                .field
                .extension_headers()
                .cloned()
                .unwrap_or_default(),
            payload,
            received_at: Instant::now(),
        }
    }

    pub(crate) fn from_fetch_object(field: FetchObjectField) -> Self {
        let (status, payload) = match field.fetch_object {
            FetchObject::Payload(payload) => (ObjectStatus::Normal, payload),
            FetchObject::Status(status) => (status, Bytes::new()),
        };
        Self {
            location: moqt::Location {
                group_id: field.group_id,
                object_id: field.object_id,
            },
            forwarding: ForwardingPreference::Subgroup {
                subgroup_id: field.subgroup_id,
            },
            publisher_priority: field.publisher_priority,
            status,
            extension_headers: field.extension_headers,
            payload,
            received_at: Instant::now(),
        }
    }

    pub(crate) fn subgroup_key(&self) -> SubgroupKey {
        match self.forwarding {
            ForwardingPreference::Subgroup { subgroup_id } => SubgroupKey::Stream {
                group_id: self.location.group_id,
                subgroup_id,
            },
            ForwardingPreference::Datagram => SubgroupKey::Datagram {
                group_id: self.location.group_id,
            },
        }
    }

    /// draft-14 §8.1: a duplicate whose Forwarding Preference, Subgroup ID,
    /// Priority or Payload differ, or whose Status moves between Normal,
    /// End of Group and End of Track, makes the track Malformed. Extension
    /// header changes and transitions involving Does Not Exist are allowed.
    pub(crate) fn duplicate_kind(&self, other: &Self) -> DuplicateKind {
        if self.forwarding != other.forwarding
            || self.publisher_priority != other.publisher_priority
        {
            return DuplicateKind::Conflict;
        }
        if self.status == ObjectStatus::DoesNotExist || other.status == ObjectStatus::DoesNotExist {
            return DuplicateKind::Identical;
        }
        if self.status != other.status || self.payload != other.payload {
            return DuplicateKind::Conflict;
        }
        DuplicateKind::Identical
    }

    pub(crate) fn to_fetch_object_field(&self) -> FetchObjectField {
        let subgroup_id = match self.forwarding {
            ForwardingPreference::Subgroup { subgroup_id } => subgroup_id,
            ForwardingPreference::Datagram => self.location.object_id,
        };
        let fetch_object = if self.payload.is_empty() {
            FetchObject::Status(self.status)
        } else {
            FetchObject::Payload(self.payload.clone())
        };
        FetchObjectField::new(
            self.location.group_id,
            subgroup_id,
            self.location.object_id,
            self.publisher_priority,
            self.extension_headers.clone(),
            fetch_object,
        )
    }

    pub(crate) fn to_subgroup_object_field(
        &self,
        message_type: SubgroupHeaderType,
        prev_sent_object_id: Option<u64>,
    ) -> SubgroupObjectField {
        let object_id = self.location.object_id;
        let object_id_delta = match prev_sent_object_id {
            None => object_id,
            Some(prev) => object_id.saturating_sub(prev.saturating_add(1)),
        };
        let subgroup_object = if self.payload.is_empty() {
            SubgroupObject::new_status(u8::from(self.status) as u64)
        } else {
            SubgroupObject::new_payload(self.payload.clone())
        };
        SubgroupObjectField {
            message_type,
            object_id_delta,
            extension_headers: self.extension_headers.clone(),
            subgroup_object,
        }
    }

    pub(crate) fn to_object_datagram(&self, track_alias: u64) -> ObjectDatagram {
        let object_id = self.location.object_id;
        let publisher_priority = self.publisher_priority;
        let extension_headers = (!self.extension_headers.key_value_pairs.is_empty())
            .then(|| self.extension_headers.clone());
        let field = match (self.status, self.payload.is_empty(), extension_headers) {
            (ObjectStatus::Normal, _, None) => DatagramField::Payload0x00 {
                object_id,
                publisher_priority,
                payload: self.payload.clone(),
            },
            (ObjectStatus::Normal, _, Some(extension_headers)) => DatagramField::Payload0x01 {
                object_id,
                publisher_priority,
                extension_headers,
                payload: self.payload.clone(),
            },
            (ObjectStatus::EndOfGroup, false, None) => DatagramField::Payload0x02WithEndOfGroup {
                object_id,
                publisher_priority,
                payload: self.payload.clone(),
            },
            (ObjectStatus::EndOfGroup, false, Some(extension_headers)) => {
                DatagramField::Payload0x03WithEndOfGroup {
                    object_id,
                    publisher_priority,
                    extension_headers,
                    payload: self.payload.clone(),
                }
            }
            (status, _, None) => DatagramField::Status0x20 {
                object_id,
                publisher_priority,
                status,
            },
            (status, _, Some(extension_headers)) => DatagramField::Status0x21 {
                object_id,
                publisher_priority,
                extension_headers,
                status,
            },
        };
        ObjectDatagram::new(track_alias, self.location.group_id, field)
    }

    fn status_from_code(code: u64) -> anyhow::Result<ObjectStatus> {
        u8::try_from(code)
            .ok()
            .and_then(|code| ObjectStatus::try_from(code).ok())
            .ok_or_else(|| anyhow::anyhow!("invalid object status {code:#x}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::relay::tests::harness::fixtures::cached_object::{
        datagram_object, status_object, stream_object, stream_object_in_subgroup,
        stream_object_with_payload, subgroup_stream,
    };
    use moqt::{KeyValuePair, SubgroupHeader, VariantType};

    fn extension_headers() -> ExtensionHeaders {
        ExtensionHeaders::new(vec![KeyValuePair {
            key: 2,
            value: VariantType::Even(7),
        }])
    }

    #[test]
    fn identical_duplicate_is_identical() {
        // Arrange / Act / Assert
        assert_eq!(
            stream_object(0, 0).duplicate_kind(&stream_object(0, 0)),
            DuplicateKind::Identical
        );
    }

    #[test]
    fn differing_payload_is_a_conflict() {
        // Arrange
        let first = stream_object_with_payload(0, 0, Bytes::from_static(b"a"));
        let second = stream_object_with_payload(0, 0, Bytes::from_static(b"b"));
        // Act / Assert
        assert_eq!(first.duplicate_kind(&second), DuplicateKind::Conflict);
    }

    #[test]
    fn differing_priority_is_a_conflict() {
        // Arrange
        let first = stream_object(0, 0);
        let second = CachedObject {
            publisher_priority: first.publisher_priority + 1,
            ..stream_object(0, 0)
        };
        // Act / Assert
        assert_eq!(first.duplicate_kind(&second), DuplicateKind::Conflict);
    }

    #[test]
    fn differing_subgroup_or_forwarding_preference_is_a_conflict() {
        // Arrange / Act / Assert
        assert_eq!(
            stream_object(0, 0).duplicate_kind(&stream_object_in_subgroup(0, 1, 0)),
            DuplicateKind::Conflict
        );
        assert_eq!(
            stream_object(0, 0).duplicate_kind(&datagram_object(0, 0)),
            DuplicateKind::Conflict
        );
    }

    #[test]
    fn differing_extension_headers_are_not_a_conflict() {
        // Arrange: §8.1 allows extension headers to be added, removed or updated
        let first = stream_object(0, 0);
        let second = CachedObject {
            extension_headers: extension_headers(),
            ..stream_object(0, 0)
        };
        // Act / Assert
        assert_eq!(first.duplicate_kind(&second), DuplicateKind::Identical);
    }

    #[test]
    fn transition_to_or_from_does_not_exist_is_not_a_conflict() {
        // Arrange
        let normal = stream_object(0, 0);
        let missing = status_object(0, 0, ObjectStatus::DoesNotExist);
        // Act / Assert: either arrival order is a tolerated duplicate
        assert_eq!(normal.duplicate_kind(&missing), DuplicateKind::Identical);
        assert_eq!(missing.duplicate_kind(&normal), DuplicateKind::Identical);
    }

    #[test]
    fn transition_between_normal_and_end_of_group_is_a_conflict() {
        // Arrange
        let normal = stream_object_with_payload(0, 0, Bytes::new());
        let end_of_group = status_object(0, 0, ObjectStatus::EndOfGroup);
        // Act / Assert
        assert_eq!(
            normal.duplicate_kind(&end_of_group),
            DuplicateKind::Conflict
        );
    }

    #[test]
    fn subgroup_object_with_invalid_status_code_is_rejected() {
        // Arrange
        let message_type =
            SubgroupHeader::new(0, 0, moqt::SubgroupId::Value(0), 0, false, false).message_type;
        let field = SubgroupObjectField {
            message_type,
            object_id_delta: 0,
            extension_headers: ExtensionHeaders::default(),
            subgroup_object: SubgroupObject::new_status(0x2),
        };
        // Act / Assert
        assert!(CachedObject::from_subgroup_object(&subgroup_stream(0, 0), 0, field).is_err());
    }

    #[test]
    fn end_of_group_datagram_with_payload_keeps_both_on_round_trip() {
        // Arrange: Type 0x06 carries a payload, an implicit object id and End of Group
        let datagram = ObjectDatagram::new(
            9,
            4,
            DatagramField::Payload0x06WithEndOfGroup {
                publisher_priority: 3,
                payload: Bytes::from_static(b"last"),
            },
        );
        // Act
        let cached = CachedObject::from_datagram(2, datagram);
        let regenerated = cached.to_object_datagram(11);
        // Assert: canonical status plus payload, re-emitted with an explicit id
        assert_eq!(cached.status, ObjectStatus::EndOfGroup);
        assert_eq!(cached.payload, Bytes::from_static(b"last"));
        assert_eq!(regenerated.track_alias, 11);
        assert_eq!(regenerated.group_id, 4);
        assert!(matches!(
            regenerated.field,
            DatagramField::Payload0x02WithEndOfGroup {
                object_id: 2,
                publisher_priority: 3,
                ..
            }
        ));
    }

    #[test]
    fn status_datagram_round_trips_as_status_type() {
        // Arrange
        let cached = status_object(0, 5, ObjectStatus::DoesNotExist);
        let cached = CachedObject {
            forwarding: ForwardingPreference::Datagram,
            ..cached
        };
        // Act / Assert
        assert!(matches!(
            cached.to_object_datagram(0).field,
            DatagramField::Status0x20 {
                object_id: 5,
                status: ObjectStatus::DoesNotExist,
                ..
            }
        ));
    }

    #[test]
    fn fetch_field_of_datagram_object_uses_object_id_as_subgroup_id() {
        // Arrange / Act
        let field = datagram_object(1, 7).to_fetch_object_field();
        // Assert: §10.4.4
        assert_eq!(field.subgroup_id, 7);
        assert_eq!(field.object_id, 7);
    }

    #[test]
    fn empty_payload_is_encoded_as_explicit_status() {
        // Arrange: a zero-length Normal object must encode Object Status 0x0
        let cached = stream_object_with_payload(0, 0, Bytes::new());
        let message_type =
            SubgroupHeader::new(0, 0, moqt::SubgroupId::Value(0), 0, true, false).message_type;
        // Act
        let fetch_field = cached.to_fetch_object_field();
        let subgroup_field = cached.to_subgroup_object_field(message_type, None);
        // Assert
        assert_eq!(
            fetch_field.fetch_object,
            FetchObject::Status(ObjectStatus::Normal)
        );
        assert_eq!(
            subgroup_field.subgroup_object,
            SubgroupObject::new_status(ObjectStatus::Normal as u64)
        );
    }

    #[test]
    fn subgroup_object_delta_is_relative_to_the_previously_sent_object() {
        // Arrange
        let message_type =
            SubgroupHeader::new(0, 0, moqt::SubgroupId::Value(0), 0, true, false).message_type;
        let first = stream_object(0, 7).to_subgroup_object_field(message_type, None);
        let second = stream_object(0, 9).to_subgroup_object_field(message_type, Some(7));
        // Act
        let first_id = first.resolve_object_id(None);
        let second_id = second.resolve_object_id(Some(first_id));
        // Assert
        assert_eq!(first_id, 7);
        assert_eq!(second_id, 9);
        assert_eq!(first.message_type, message_type);
    }

    #[test]
    fn fetch_object_converts_one_to_one() {
        // Arrange
        let field = FetchObjectField::new(
            3,
            1,
            4,
            200,
            extension_headers(),
            FetchObject::Payload(Bytes::from_static(b"p")),
        );
        // Act
        let cached = CachedObject::from_fetch_object(field.clone());
        // Assert
        assert_eq!(
            cached.location,
            moqt::Location {
                group_id: 3,
                object_id: 4
            }
        );
        assert_eq!(
            cached.forwarding,
            ForwardingPreference::Subgroup { subgroup_id: 1 }
        );
        assert_eq!(cached.publisher_priority, 200);
        assert_eq!(cached.to_fetch_object_field(), field);
    }
}
