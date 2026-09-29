use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::data_plane::object::{extension_headers::ExtensionHeaders, object_status::ObjectStatus},
};

const EXTENSIONS_PRESENT: u64 = 0x01;
const END_OF_GROUP: u64 = 0x02;
const OBJECT_ID_ABSENT: u64 = 0x04;
const STATUS: u64 = 0x20;

// Deviation from draft-14 §10.3.1: End of Group types carry this byte before the
// Object ID. Kept so the wire format stays compatible with existing peers.
const END_OF_GROUP_PREFIX: &[u8] = &[0x01];

// draft-14 §10.3.1 Table 6: status types never omit the Object ID and never end the group.
fn is_defined_type(message_type: u64) -> bool {
    matches!(message_type, 0x00..=0x07 | 0x20 | 0x21)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectDatagramPayload {
    Payload(Bytes),
    Status(ObjectStatus),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatagramField {
    pub object_id: Option<u64>,
    pub publisher_priority: u8,
    pub extension_headers: Option<ExtensionHeaders>,
    pub end_of_group: bool,
    pub payload: ObjectDatagramPayload,
}

impl DatagramField {
    /// draft-14 §10.3.1: when the Object ID field is omitted, the Object ID is 0.
    pub fn resolve_object_id(&self) -> u64 {
        self.object_id.unwrap_or(0)
    }

    pub(crate) fn decode(message_type: u64, data: &mut BytesMut) -> Option<Self> {
        if !is_defined_type(message_type) {
            tracing::error!("Invalid message type: {}", message_type);
            return None;
        }
        let has = |flag: u64| message_type & flag != 0;
        let end_of_group = has(END_OF_GROUP);
        if end_of_group {
            if !data.starts_with(END_OF_GROUP_PREFIX) {
                return None;
            }
            data.advance(END_OF_GROUP_PREFIX.len());
        }
        let object_id = if has(OBJECT_ID_ABSENT) {
            None
        } else {
            Some(data.try_get_varint().log_context("object id").ok()?)
        };
        let publisher_priority = data.try_get_u8().log_context("publisher priority").ok()?;
        let extension_headers = if has(EXTENSIONS_PRESENT) {
            Some(ExtensionHeaders::decode(data)?)
        } else {
            None
        };
        let payload = if has(STATUS) {
            let status = data.try_get_u8().log_context("status").ok()?;
            ObjectDatagramPayload::Status(ObjectStatus::try_from(status).ok()?)
        } else {
            ObjectDatagramPayload::Payload(data.split().freeze())
        };
        Some(Self {
            object_id,
            publisher_priority,
            extension_headers,
            end_of_group,
            payload,
        })
    }

    pub(crate) fn encode(&self) -> anyhow::Result<(u64, BytesMut)> {
        let message_type = (u64::from(self.extension_headers.is_some()) * EXTENSIONS_PRESENT)
            | (u64::from(self.end_of_group) * END_OF_GROUP)
            | (u64::from(self.object_id.is_none()) * OBJECT_ID_ABSENT)
            | (u64::from(matches!(self.payload, ObjectDatagramPayload::Status(_))) * STATUS);
        if !is_defined_type(message_type) {
            anyhow::bail!(
                "undefined OBJECT_DATAGRAM type {message_type:#x}: a status needs an Object ID and cannot end the group"
            );
        }
        let mut buf = BytesMut::new();
        if self.end_of_group {
            buf.put_slice(END_OF_GROUP_PREFIX);
        }
        if let Some(object_id) = self.object_id {
            buf.put_varint(object_id);
        }
        buf.put_u8(self.publisher_priority);
        if let Some(extension_headers) = &self.extension_headers {
            buf.unsplit(extension_headers.encode());
        }
        match &self.payload {
            ObjectDatagramPayload::Payload(payload) => buf.extend_from_slice(payload),
            ObjectDatagramPayload::Status(status) => buf.put_u8(*status as u8),
        }
        Ok((message_type, buf))
    }
}

#[cfg(test)]
mod tests {
    use bytes::{Bytes, BytesMut};

    use super::{DatagramField, ObjectDatagramPayload};
    use crate::modules::moqt::data_plane::object::{
        extension_headers::ExtensionHeaders, object_status::ObjectStatus,
    };

    const PAYLOAD: &[u8] = &[0xAA, 0xBB];
    const ENCODED_STATUS: &[u8] = &[0x01];

    fn payload(object_id: Option<u64>) -> DatagramField {
        DatagramField {
            object_id,
            publisher_priority: 0x80,
            extension_headers: None,
            end_of_group: false,
            payload: ObjectDatagramPayload::Payload(Bytes::from_static(PAYLOAD)),
        }
    }

    fn status(object_id: Option<u64>) -> DatagramField {
        DatagramField {
            payload: ObjectDatagramPayload::Status(ObjectStatus::DoesNotExist),
            ..payload(object_id)
        }
    }

    fn with_extensions(field: DatagramField) -> DatagramField {
        let mut extension_headers = ExtensionHeaders::default();
        extension_headers.push_prior_group_id_gap(3);
        DatagramField {
            extension_headers: Some(extension_headers),
            ..field
        }
    }

    fn ending_group(field: DatagramField) -> DatagramField {
        DatagramField {
            end_of_group: true,
            ..field
        }
    }

    #[test]
    fn every_defined_type_round_trips_with_its_wire_bytes() {
        // Arrange
        let id: &[u8] = &[0x05];
        let priority: &[u8] = &[0x80];
        let eog_prefix: &[u8] = &[0x01];
        let ext: &[u8] = &[0x02, 0x3C, 0x03];
        let cases = [
            (payload(Some(5)), 0x00, [id, priority, PAYLOAD].concat()),
            (
                with_extensions(payload(Some(5))),
                0x01,
                [id, priority, ext, PAYLOAD].concat(),
            ),
            (
                ending_group(payload(Some(5))),
                0x02,
                [eog_prefix, id, priority, PAYLOAD].concat(),
            ),
            (
                ending_group(with_extensions(payload(Some(5)))),
                0x03,
                [eog_prefix, id, priority, ext, PAYLOAD].concat(),
            ),
            (payload(None), 0x04, [priority, PAYLOAD].concat()),
            (
                with_extensions(payload(None)),
                0x05,
                [priority, ext, PAYLOAD].concat(),
            ),
            (
                ending_group(payload(None)),
                0x06,
                [eog_prefix, priority, PAYLOAD].concat(),
            ),
            (
                ending_group(with_extensions(payload(None))),
                0x07,
                [eog_prefix, priority, ext, PAYLOAD].concat(),
            ),
            (
                status(Some(5)),
                0x20,
                [id, priority, ENCODED_STATUS].concat(),
            ),
            (
                with_extensions(status(Some(5))),
                0x21,
                [id, priority, ext, ENCODED_STATUS].concat(),
            ),
        ];
        for (field, expected_type, expected_bytes) in cases {
            // Act
            let (message_type, encoded) = field.encode().unwrap();
            let mut received = BytesMut::from(&expected_bytes[..]);
            let decoded = DatagramField::decode(expected_type, &mut received).unwrap();
            // Assert
            assert_eq!(message_type, expected_type);
            assert_eq!(encoded.to_vec(), expected_bytes, "type {expected_type:#x}");
            assert_eq!(decoded, field, "type {expected_type:#x}");
            assert!(received.is_empty(), "type {expected_type:#x}");
        }
    }

    #[test]
    fn undefined_types_are_rejected_on_decode() {
        for message_type in [0x08, 0x22, 0x23, 0x24, 0x26, 0x40] {
            // Arrange
            let mut received = BytesMut::from(&[0x01, 0x05, 0x80, 0x01][..]);
            // Act / Assert
            assert!(
                DatagramField::decode(message_type, &mut received).is_none(),
                "type {message_type:#x}"
            );
        }
    }

    #[test]
    fn status_that_ends_the_group_or_omits_the_object_id_is_rejected_on_encode() {
        // Arrange
        let fields = [ending_group(status(Some(5))), status(None)];
        for field in fields {
            // Act / Assert
            assert!(field.encode().is_err(), "{field:?}");
        }
    }

    #[test]
    fn present_object_id_is_returned_as_is() {
        // Arrange
        let field = payload(Some(7));
        // Act / Assert
        assert_eq!(field.resolve_object_id(), 7);
    }

    #[test]
    fn omitted_object_id_resolves_to_zero() {
        // Arrange
        let field = ending_group(payload(None));
        // Act / Assert
        assert_eq!(field.resolve_object_id(), 0);
    }
}
