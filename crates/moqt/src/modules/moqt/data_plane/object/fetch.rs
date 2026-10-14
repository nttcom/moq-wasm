use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::modules::extensions::buf_get_ext::BufGetExt;
use crate::modules::extensions::buf_put_ext::BufPutExt;
use crate::modules::moqt::data_plane::object::decode_error::DecodeError;
use crate::modules::moqt::data_plane::object::extension_headers::ExtensionHeaders;
use crate::modules::moqt::data_plane::object::object_status::ObjectStatus;
use crate::modules::moqt::{
    control_plane::control_messages::messages::parameters::location::Location,
    data_plane::object::canonical_object::{ForwardingPreference, Object},
};

// FETCH_HEADER {
//   Type (i) = 0x5,
//   Request ID (i),
// }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchHeader {
    pub request_id: u64,
}

impl FetchHeader {
    pub const TYPE: u64 = 0x5;

    pub fn new(request_id: u64) -> Self {
        Self { request_id }
    }

    pub fn decode(cursor: &mut std::io::Cursor<&[u8]>) -> Result<Self, DecodeError> {
        let message_type = cursor.try_get_varint()?;
        if message_type != Self::TYPE {
            return Err(DecodeError::Fatal(format!(
                "Invalid message type for FetchHeader: {message_type:#x}"
            )));
        }
        let request_id = cursor.try_get_varint()?;
        Ok(Self { request_id })
    }

    pub fn encode(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_varint(Self::TYPE);
        buf.put_varint(self.request_id);
        buf
    }
}

// https://www.ietf.org/archive/id/draft-ietf-moq-transport-14.html#section-10.4.4-3
// {
//   Group ID (i),
//   Subgroup ID (i),
//   Object ID (i),
//   Publisher Priority (8),
//   Extension Headers Length (i),
//   [Extension headers (...)],
//   Object Payload Length (i),
//   [Object Status (i)],
//   Object Payload (..),
// }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchObject {
    Payload(Bytes),
    Status(ObjectStatus),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchObjectField {
    pub group_id: u64,
    pub subgroup_id: u64,
    pub object_id: u64,
    pub publisher_priority: u8,
    pub extension_headers: ExtensionHeaders,
    pub fetch_object: FetchObject,
}

impl FetchObjectField {
    /// A FETCH object has no forwarding preference: a datagram object is sent
    /// with its Object ID as the Subgroup ID (draft-14 §10.4.4), and a
    /// zero-length object as its status.
    pub(crate) fn from_object(object: &Object) -> Self {
        let subgroup_id = match object.forwarding {
            ForwardingPreference::Subgroup { subgroup_id } => subgroup_id,
            ForwardingPreference::Datagram => object.location.object_id,
        };
        let fetch_object = if object.payload.is_empty() {
            FetchObject::Status(object.status)
        } else {
            FetchObject::Payload(object.payload.clone())
        };
        Self::new(
            object.location.group_id,
            subgroup_id,
            object.location.object_id,
            object.publisher_priority,
            object.extension_headers.clone(),
            fetch_object,
        )
    }

    pub(crate) fn into_object(self) -> Object {
        let (status, payload) = match self.fetch_object {
            FetchObject::Payload(payload) => (ObjectStatus::Normal, payload),
            FetchObject::Status(status) => (status, Bytes::new()),
        };
        Object {
            location: Location {
                group_id: self.group_id,
                object_id: self.object_id,
            },
            forwarding: ForwardingPreference::Subgroup {
                subgroup_id: self.subgroup_id,
            },
            publisher_priority: self.publisher_priority,
            status,
            extension_headers: self.extension_headers,
            payload,
        }
    }

    pub fn new(
        group_id: u64,
        subgroup_id: u64,
        object_id: u64,
        publisher_priority: u8,
        extension_headers: ExtensionHeaders,
        fetch_object: FetchObject,
    ) -> Self {
        Self {
            group_id,
            subgroup_id,
            object_id,
            publisher_priority,
            extension_headers,
            fetch_object,
        }
    }

    pub fn decode(buf: &mut BytesMut) -> Result<Self, DecodeError> {
        let mut cursor = std::io::Cursor::<&[u8]>::new(buf);
        let group_id = cursor.try_get_varint()?;
        let subgroup_id = cursor.try_get_varint()?;
        let object_id = cursor.try_get_varint()?;
        let publisher_priority = cursor.try_get_u8()?;
        let extension_headers =
            ExtensionHeaders::decode(&mut cursor).ok_or(DecodeError::NeedMoreData)?;
        let payload_length = cursor.try_get_varint()?;
        let fetch_object = if payload_length == 0 {
            let status_value = cursor.try_get_varint()?;
            let status = ObjectStatus::try_from(status_value as u8).map_err(|_| {
                DecodeError::Fatal(format!("Invalid Object Status: {status_value:#x}"))
            })?;
            let _ = buf.split_to(cursor.position() as usize);
            FetchObject::Status(status)
        } else {
            if cursor.remaining() < payload_length as usize {
                return Err(DecodeError::NeedMoreData);
            }
            let _ = buf.split_to(cursor.position() as usize);
            let payload = Bytes::copy_from_slice(&buf[..payload_length as usize]);
            buf.advance(payload_length as usize);
            FetchObject::Payload(payload)
        };
        Ok(Self {
            group_id,
            subgroup_id,
            object_id,
            publisher_priority,
            extension_headers,
            fetch_object,
        })
    }

    pub fn encode(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_varint(self.group_id);
        buf.put_varint(self.subgroup_id);
        buf.put_varint(self.object_id);
        buf.put_u8(self.publisher_priority);
        let ext_headers_buf = self.extension_headers.encode();
        buf.extend_from_slice(&ext_headers_buf);
        match &self.fetch_object {
            FetchObject::Payload(payload) => {
                buf.put_varint(payload.len() as u64);
                buf.put_slice(payload);
            }
            FetchObject::Status(status) => {
                buf.put_varint(0);
                buf.put_varint(u8::from(*status) as u64);
            }
        }
        buf
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use super::super::*;
        use bytes::Bytes;

        #[test]
        fn fetch_header_packetize() {
            let header = FetchHeader::new(42);
            let result = header.encode();
            let expected = [
                0x05, // Type
                42,   // Request ID
            ];
            assert_eq!(result.as_ref(), expected.as_slice());
        }

        #[test]
        fn fetch_header_depacketize() {
            let bytes = [0x05u8, 42];
            let mut cursor = std::io::Cursor::new(&bytes[..]);
            let result = FetchHeader::decode(&mut cursor).unwrap();
            let expected = FetchHeader::new(42);
            assert_eq!(result, expected);
        }

        #[test]
        fn fetch_header_round_trip() {
            let expected = FetchHeader::new(12345);
            let buf = expected.encode();
            let mut cursor = std::io::Cursor::new(&buf[..]);
            let result = FetchHeader::decode(&mut cursor).unwrap();
            assert_eq!(result, expected);
        }

        fn sample_fields(object: FetchObject) -> FetchObjectField {
            FetchObjectField::new(
                1,   // group_id
                2,   // subgroup_id
                3,   // object_id
                128, // publisher_priority
                ExtensionHeaders::default(),
                object,
            )
        }

        #[test]
        fn packetize_with_payload() {
            let fields = sample_fields(FetchObject::Payload(Bytes::from_static(b"abcd")));
            let result = fields.encode();
            let expected = vec![
                1,   // Group ID
                2,   // Subgroup ID
                3,   // Object ID
                128, // Publisher Priority
                0,   // Extension Headers Length (= 0, 拡張なし)
                4,   // Object Payload Length
                b'a', b'b', b'c', b'd', // Payload
            ];
            assert_eq!(result.as_ref(), expected.as_slice());
        }

        #[test]
        fn depacketize_with_payload() {
            let bytes: [u8; 10] = [1, 2, 3, 128, 0, 4, b'a', b'b', b'c', b'd'];
            let mut buf = bytes::BytesMut::with_capacity(bytes.len());
            buf.extend_from_slice(&bytes);
            let result = FetchObjectField::decode(&mut buf).unwrap();
            let expected = sample_fields(FetchObject::Payload(Bytes::from_static(b"abcd")));
            assert_eq!(result, expected);
        }

        #[test]
        fn round_trip_with_payload() {
            let expected = sample_fields(FetchObject::Payload(Bytes::from_static(b"hello")));
            let mut buf = expected.encode();
            let result = FetchObjectField::decode(&mut buf).unwrap();
            assert_eq!(result, expected);
        }

        #[test]
        fn round_trip_with_status() {
            let expected = sample_fields(FetchObject::Status(ObjectStatus::DoesNotExist));
            let mut buf = expected.encode();
            let result = FetchObjectField::decode(&mut buf).unwrap();
            assert_eq!(result, expected);
        }
    }

    mod failure {
        use super::super::*;

        #[test]
        fn depacketize_split_payload_length_varint_needs_more_data() {
            // Arrange
            let bytes = [1u8, 2, 3, 128, 0, 0x40];
            let mut buf = BytesMut::from(&bytes[..]);

            // Act
            let result = FetchObjectField::decode(&mut buf);

            // Assert
            assert_eq!(result, Err(DecodeError::NeedMoreData));
        }

        #[test]
        fn depacketize_invalid_type() {
            let bytes = [0x10u8, 42];
            let mut cursor = std::io::Cursor::new(&bytes[..]);
            let result = FetchHeader::decode(&mut cursor);
            assert!(matches!(result, Err(DecodeError::Fatal(_))));
        }
    }
}

#[cfg(test)]
mod conversion_tests {
    use bytes::Bytes;

    use super::{FetchObject, FetchObjectField};
    use crate::modules::moqt::{
        control_plane::control_messages::messages::parameters::location::Location,
        data_plane::object::{
            canonical_object::{ForwardingPreference, Object},
            extension_headers::ExtensionHeaders,
            object_status::ObjectStatus,
        },
    };

    fn object(forwarding: ForwardingPreference, payload: &'static [u8]) -> Object {
        Object {
            location: Location {
                group_id: 3,
                object_id: 7,
            },
            forwarding,
            publisher_priority: 200,
            status: ObjectStatus::Normal,
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::from_static(payload),
        }
    }

    #[test]
    fn a_subgroup_object_round_trips() {
        // Arrange
        let sent = object(ForwardingPreference::Subgroup { subgroup_id: 1 }, b"p");

        // Act
        let received = FetchObjectField::from_object(&sent).into_object();

        // Assert
        assert_eq!(received, sent);
    }

    #[test]
    fn a_datagram_object_takes_its_object_id_as_subgroup_id() {
        // Act
        let field = FetchObjectField::from_object(&object(ForwardingPreference::Datagram, b"p"));

        // Assert: §10.4.4
        assert_eq!(field.subgroup_id, 7);
    }

    #[test]
    fn a_zero_length_object_is_sent_as_its_status() {
        // Act
        let field = FetchObjectField::from_object(&object(
            ForwardingPreference::Subgroup { subgroup_id: 0 },
            b"",
        ));

        // Assert
        assert_eq!(
            field.fetch_object,
            FetchObject::Status(ObjectStatus::Normal)
        );
    }
}
