use anyhow::Context;
use bytes::BytesMut;

use crate::modules::extensions::buf_get_ext::BufGetExt;
use crate::modules::extensions::buf_put_ext::BufPutExt;
use crate::modules::moqt::{
    control_plane::control_messages::messages::parameters::location::Location,
    data_plane::object::{
        canonical_object::{ForwardingPreference, Object},
        datagram_field::{DatagramField, ObjectDatagramPayload},
        object_status::ObjectStatus,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectDatagram {
    pub track_alias: u64,
    pub group_id: u64,
    pub field: DatagramField,
}

impl ObjectDatagram {
    /// draft-14 §10.3.1: a zero-length object encodes its status explicitly,
    /// and a payload that ends its group uses an End of Group type.
    pub(crate) fn from_object(track_alias: u64, object: &Object) -> Self {
        let payload = if object.payload.is_empty() {
            ObjectDatagramPayload::Status(object.status)
        } else {
            ObjectDatagramPayload::Payload(object.payload.clone())
        };
        let field = DatagramField {
            object_id: Some(object.location.object_id),
            publisher_priority: object.publisher_priority,
            extension_headers: (!object.extension_headers.key_value_pairs.is_empty())
                .then(|| object.extension_headers.clone()),
            end_of_group: !object.payload.is_empty() && object.ends_group,
            payload,
        };
        Self::new(track_alias, object.location.group_id, field)
    }

    pub(crate) fn into_object(self) -> Object {
        let object_id = self.field.resolve_object_id();
        let field = self.field;
        let (status, payload) = match field.payload {
            ObjectDatagramPayload::Payload(payload) => (ObjectStatus::Normal, payload),
            ObjectDatagramPayload::Status(status) => (status, bytes::Bytes::new()),
        };
        Object {
            location: Location {
                group_id: self.group_id,
                object_id,
            },
            forwarding: ForwardingPreference::Datagram,
            publisher_priority: field.publisher_priority,
            status,
            extension_headers: field.extension_headers.unwrap_or_default(),
            payload,
            ends_group: field.end_of_group,
        }
    }

    pub fn new(track_alias: u64, group_id: u64, field: DatagramField) -> Self {
        Self {
            track_alias,
            group_id,
            field,
        }
    }

    pub fn decode(buf: &mut BytesMut) -> anyhow::Result<Self> {
        let message_type = buf.try_get_varint().context("datagram type")?;
        let track_alias = buf.try_get_varint().context("track alias")?;
        let group_id = buf.try_get_varint().context("group id")?;
        let field = DatagramField::decode(message_type, buf)?;

        Ok(Self {
            track_alias,
            group_id,
            field,
        })
    }

    pub fn encode(&self) -> anyhow::Result<BytesMut> {
        let mut buf = BytesMut::new();
        let (message_type, field_bytes) = self.field.encode()?;
        buf.put_varint(message_type);
        buf.put_varint(self.track_alias);
        buf.put_varint(self.group_id);
        buf.unsplit(field_bytes);

        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::modules::moqt::data_plane::object::{
        datagram_field::{DatagramField, ObjectDatagramPayload},
        object_datagram::ObjectDatagram,
    };

    #[test]
    fn type_track_alias_and_group_id_precede_the_field() {
        // Arrange
        let object = ObjectDatagram::new(
            1,
            2,
            DatagramField {
                object_id: Some(3),
                publisher_priority: 0x80,
                extension_headers: None,
                end_of_group: false,
                payload: ObjectDatagramPayload::Payload(Bytes::from_static(&[0xAA])),
            },
        );
        // Act
        let mut encoded = object.encode().unwrap();
        let encoded_bytes = encoded.to_vec();
        let decoded = ObjectDatagram::decode(&mut encoded).unwrap();
        // Assert
        assert_eq!(encoded_bytes, [0x00, 0x01, 0x02, 0x03, 0x80, 0xAA]);
        assert_eq!(decoded, object);
    }
}

#[cfg(test)]
mod conversion_tests {
    use bytes::Bytes;

    use super::ObjectDatagram;
    use crate::modules::moqt::{
        control_plane::control_messages::messages::parameters::location::Location,
        data_plane::object::{
            canonical_object::{ForwardingPreference, Object},
            datagram_field::{DatagramField, ObjectDatagramPayload},
            extension_headers::ExtensionHeaders,
            object_status::ObjectStatus,
        },
    };

    fn object(object_id: u64, status: ObjectStatus, payload: &'static [u8]) -> Object {
        Object {
            location: Location {
                group_id: 4,
                object_id,
            },
            forwarding: ForwardingPreference::Datagram,
            publisher_priority: 3,
            status,
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::from_static(payload),
            ends_group: false,
        }
    }

    #[test]
    fn end_of_group_type_with_payload_is_a_normal_object_ending_its_group() {
        // Arrange
        let received = ObjectDatagram::new(
            0,
            4,
            DatagramField {
                object_id: None,
                publisher_priority: 3,
                extension_headers: None,
                end_of_group: true,
                payload: ObjectDatagramPayload::Payload(Bytes::from_static(b"last")),
            },
        );

        // Act
        let object = received.into_object();
        let regenerated = ObjectDatagram::from_object(11, &object);

        // Assert: Normal per §10.2.1.1; re-emitted with an explicit object id and
        // its End of Group type
        assert_eq!(object, object_at_zero_ending_the_group());
        assert_eq!(regenerated.track_alias, 11);
        assert_eq!(
            regenerated.field,
            DatagramField {
                object_id: Some(0),
                publisher_priority: 3,
                extension_headers: None,
                end_of_group: true,
                payload: ObjectDatagramPayload::Payload(Bytes::from_static(b"last")),
            }
        );
    }

    fn object_at_zero_ending_the_group() -> Object {
        Object {
            ends_group: true,
            ..object(0, ObjectStatus::Normal, b"last")
        }
    }

    #[test]
    fn status_object_is_sent_as_a_status_type() {
        // Act
        let field =
            ObjectDatagram::from_object(0, &object(5, ObjectStatus::DoesNotExist, b"")).field;

        // Assert
        assert_eq!(
            field,
            DatagramField {
                object_id: Some(5),
                publisher_priority: 3,
                extension_headers: None,
                end_of_group: false,
                payload: ObjectDatagramPayload::Status(ObjectStatus::DoesNotExist),
            }
        );
    }

    #[test]
    fn zero_length_normal_object_is_sent_as_an_explicit_status() {
        // Act
        let field = ObjectDatagram::from_object(0, &object(4, ObjectStatus::Normal, b"")).field;

        // Assert: §10.2.1.1
        assert_eq!(
            field.payload,
            ObjectDatagramPayload::Status(ObjectStatus::Normal)
        );
    }
}
