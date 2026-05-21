use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::modules::extensions::buf_get_ext::BufGetExt;
use crate::modules::extensions::buf_put_ext::BufPutExt;
use crate::modules::extensions::result_ext::ResultExt;
use crate::modules::moqt::data_plane::object::decode_error::DecodeError;
use crate::modules::moqt::data_plane::object::extension_headers::ExtensionHeaders;
use crate::modules::moqt::data_plane::object::object_status::ObjectStatus;

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
        let message_type = cursor
            .try_get_varint()
            .log_context("Fetch Header Message Type")
            .map_err(|_| DecodeError::NeedMoreData)?;
        if message_type != Self::TYPE {
            return Err(DecodeError::Fatal(format!(
                "Invalid message type for FetchHeader: {message_type:#x}"
            )));
        }
        let request_id = cursor
            .try_get_varint()
            .log_context("Fetch Header Request ID")
            .map_err(|_| DecodeError::NeedMoreData)?;
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
pub struct FetchObjectFields {
    pub group_id: u64,
    pub subgroup_id: u64,
    pub object_id: u64,
    pub publisher_priority: u8,
    pub extension_headers: ExtensionHeaders,
    pub fetch_object: FetchObject,
}

impl FetchObjectFields {
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
        let group_id = cursor
            .try_get_varint()
            .log_context("Fetch Object Group ID")
            .map_err(|_| DecodeError::NeedMoreData)?;
        let subgroup_id = cursor
            .try_get_varint()
            .log_context("Fetch Object Subgroup ID")
            .map_err(|_| DecodeError::NeedMoreData)?;
        let object_id = cursor
            .try_get_varint()
            .log_context("Fetch Object Object ID")
            .map_err(|_| DecodeError::NeedMoreData)?;
        let publisher_priority = cursor
            .try_get_u8()
            .log_context("Fetch Object Publisher Priority")
            .map_err(|_| DecodeError::NeedMoreData)?;
        let extension_headers =
            ExtensionHeaders::decode(&mut cursor).ok_or(DecodeError::NeedMoreData)?;
        let payload_length = cursor
            .try_get_varint()
            .log_context("Fetch Object Payload Length")
            .map_err(|_| DecodeError::NeedMoreData)?;
        let fetch_object = if payload_length == 0 {
            let status_value = cursor
                .try_get_varint()
                .log_context("Fetch Object Status")
                .map_err(|_| DecodeError::NeedMoreData)?;
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
            let payload = buf.split_to(payload_length as usize).freeze();
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

        fn sample_fields(object: FetchObject) -> FetchObjectFields {
            FetchObjectFields::new(
                1,   // group_id
                2,   // subgroup_id
                3,   // object_id
                128, // publisher_priority
                ExtensionHeaders {
                    prior_group_id_gap: vec![],
                    prior_object_id_gap: vec![],
                    immutable_extensions: vec![],
                },
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
            let result = FetchObjectFields::decode(&mut buf).unwrap();
            let expected = sample_fields(FetchObject::Payload(Bytes::from_static(b"abcd")));
            assert_eq!(result, expected);
        }

        #[test]
        fn round_trip_with_payload() {
            let expected = sample_fields(FetchObject::Payload(Bytes::from_static(b"hello")));
            let mut buf = expected.encode();
            let result = FetchObjectFields::decode(&mut buf).unwrap();
            assert_eq!(result, expected);
        }

        #[test]
        fn round_trip_with_status() {
            let expected = sample_fields(FetchObject::Status(ObjectStatus::DoesNotExist));
            let mut buf = expected.encode();
            let result = FetchObjectFields::decode(&mut buf).unwrap();
            assert_eq!(result, expected);
        }
    }

    mod failure {
        use super::super::*;

        #[test]
        fn depacketize_invalid_type() {
            let bytes = [0x10u8, 42];
            let mut cursor = std::io::Cursor::new(&bytes[..]);
            let result = FetchHeader::decode(&mut cursor);
            assert!(matches!(result, Err(DecodeError::Fatal(_))));
        }
    }
}
