use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::modules::extensions::buf_get_ext::BufGetExt;
use crate::modules::extensions::buf_put_ext::BufPutExt;
use crate::modules::extensions::result_ext::ResultExt;
use crate::modules::moqt::data_plane::object::decode_error::DecodeError;
use crate::modules::moqt::data_plane::object::extension_headers::ExtensionHeaders;

//  +======+===============+=============+============+==============+
//  | Type | Subgroup ID   | Subgroup ID | Extensions | Contains End |
//  +======+===============+=============+============+==============+
//  |      | Field Present | Value       | Present    | of Group     |
//  +------+---------------+-------------+------------+--------------+
//  | 0x10 | No            | 0           | No         | No           |
//  +------+---------------+-------------+------------+--------------+
//  | 0x11 | No            | 0           | Yes        | No           |
//  +------+---------------+-------------+------------+--------------+
//  | 0x12 | No            | First       | No         | No           |
//  |      |               | Object ID   |            |              |
//  +------+---------------+-------------+------------+--------------+
//  | 0x13 | No            | First       | Yes        | No           |
//  |      |               | Object ID   |            |              |
//  +------+---------------+-------------+------------+--------------+
//  | 0x14 | Yes           | N/A         | No         | No           |
//  +------+---------------+-------------+------------+--------------+
//  | 0x15 | Yes           | N/A         | Yes        | No           |
//  +------+---------------+-------------+------------+--------------+
//  | 0x18 | No            | 0           | No         | Yes          |
//  +------+---------------+-------------+------------+--------------+
//  | 0x19 | No            | 0           | Yes        | Yes          |
//  +------+---------------+-------------+------------+--------------+
//  | 0x1A | No            | First       | No         | Yes          |
//  |      |               | Object ID   |            |              |
//  +------+---------------+-------------+------------+--------------+
//  | 0x1B | No            | First       | Yes        | Yes          |
//  |      |               | Object ID   |            |              |
//  +------+---------------+-------------+------------+--------------+
//  | 0x1C | Yes           | N/A         | No         | Yes          |
//  +------+---------------+-------------+------------+--------------+
//  | 0x1D | Yes           | N/A         | Yes        | Yes          |
//  +------+---------------+-------------+------------+--------------+

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubgroupId {
    None,
    FirstObjectIdDelta,
    Value(u64),
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub struct SubgroupHeaderType(u64);

impl SubgroupHeaderType {
    fn new(value: u64) -> Option<Self> {
        (0x10..=0x1d).contains(&value).then_some(Self(value))
    }

    fn new_with_parameters(
        subgroup_id_field: SubgroupId,
        has_extension_headers: bool,
        has_end_of_group: bool,
    ) -> Self {
        let value = match subgroup_id_field {
            SubgroupId::None => {
                if has_extension_headers && has_end_of_group {
                    0x19
                } else if has_extension_headers {
                    0x11
                } else if has_end_of_group {
                    0x18
                } else {
                    0x10
                }
            }
            SubgroupId::FirstObjectIdDelta => {
                if has_extension_headers && has_end_of_group {
                    0x1B
                } else if has_extension_headers {
                    0x13
                } else if has_end_of_group {
                    0x1A
                } else {
                    0x12
                }
            }
            SubgroupId::Value(_) => {
                if has_extension_headers && has_end_of_group {
                    0x1D
                } else if has_extension_headers {
                    0x15
                } else if has_end_of_group {
                    0x1C
                } else {
                    0x14
                }
            }
        };
        Self(value)
    }

    fn get_value(&self) -> u64 {
        self.0
    }

    fn get_subgroup_id_field(
        &self,
        buf: &mut std::io::Cursor<&[u8]>,
    ) -> Result<SubgroupId, DecodeError> {
        match self.0 {
            0x10 | 0x11 | 0x18 | 0x19 => Ok(SubgroupId::None),
            0x12 | 0x13 | 0x1A | 0x1B => Ok(SubgroupId::FirstObjectIdDelta),
            0x14 | 0x15 | 0x1C | 0x1D => {
                let subgroup_id = buf
                    .try_get_varint()
                    .log_context("Subgroup Header Subgroup ID")
                    .map_err(|_| DecodeError::NeedMoreData)?;
                Ok(SubgroupId::Value(subgroup_id))
            }
            _ => {
                tracing::error!("Invalid message type: {}", self.0);
                return Err(DecodeError::Fatal(format!(
                    "Invalid message type: {}",
                    self.0
                )));
            }
        }
    }

    fn has_subgroup_id(&self) -> bool {
        matches!(self.0, 0x14 | 0x15 | 0x1C | 0x1D)
    }

    fn has_extensions(&self) -> bool {
        matches!(self.0, 0x11 | 0x13 | 0x15 | 0x19 | 0x1B | 0x1D)
    }

    fn has_end_of_group(&self) -> bool {
        matches!(self.0, 0x18..=0x1D)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubgroupObject {
    Payload { length: usize, data: Bytes },
    Status { length: usize, code: u64 },
}

impl SubgroupObject {
    pub fn new_payload(payload: Bytes) -> Self {
        Self::Payload {
            length: payload.len(),
            data: payload,
        }
    }

    pub fn new_status(status: u64) -> Self {
        Self::Status {
            length: 0,
            code: status,
        }
    }

    pub(crate) fn check_length(cursor: &mut std::io::Cursor<&[u8]>) -> Option<usize> {
        let length = cursor.try_get_varint().log_context("payload length").ok()?;
        let length = length as usize;
        if cursor.remaining() < length {
            return None;
        }
        Some(length)
    }

    pub(crate) fn decode_status(cursor: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let status = cursor.try_get_varint().log_context("status code").ok()?;
        Some(Self::Status {
            length: 0,
            code: status,
        })
    }

    pub(crate) fn decode_payload(length: usize, buf: &mut BytesMut) -> Self {
        let payload = buf.split_to(length);
        Self::Payload {
            length,
            data: payload.freeze(),
        }
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        match self {
            SubgroupObject::Payload { length, data } => {
                buf.put_varint(*length as u64);
                buf.extend_from_slice(data);
            }
            SubgroupObject::Status { length, code } => {
                buf.put_varint(*length as u64); // length 0 indicates status
                buf.put_varint(*code);
            }
        }
        buf
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubgroupHeader {
    pub message_type: SubgroupHeaderType,
    pub track_alias: u64,
    pub group_id: u64,
    pub subgroup_id: SubgroupId,
    pub publisher_priority: u8,
}

impl SubgroupHeader {
    pub fn new(
        track_alias: u64,
        group_id: u64,
        subgroup_id: SubgroupId,
        publisher_priority: u8,
        has_extension_headers: bool,
        has_end_of_group: bool,
    ) -> Self {
        let message_type = SubgroupHeaderType::new_with_parameters(
            subgroup_id.clone(),
            has_extension_headers,
            has_end_of_group,
        );
        Self {
            message_type,
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        }
    }

    pub(crate) fn decode(mut cursor: &mut std::io::Cursor<&[u8]>) -> Result<Self, DecodeError> {
        let message_type = cursor
            .try_get_varint()
            .log_context("Subgroup Header Message Type")
            .map_err(|_| DecodeError::NeedMoreData)?;
        let message_type = SubgroupHeaderType::new(message_type)
            .ok_or_else(|| DecodeError::Fatal("Invalid message type".to_string()))?;
        let track_alias = cursor
            .try_get_varint()
            .log_context("Subgroup Header Track Alias")
            .map_err(|_| DecodeError::NeedMoreData)?;
        let group_id = cursor
            .try_get_varint()
            .log_context("Subgroup Header Group ID")
            .map_err(|_| DecodeError::NeedMoreData)?;
        let subgroup_id = message_type.get_subgroup_id_field(&mut cursor)?;
        let publisher_priority = cursor
            .try_get_u8()
            .log_context("Subgroup Header Publisher Priority")
            .map_err(|_| DecodeError::NeedMoreData)?;
        Ok(Self {
            message_type,
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        })
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_varint(self.message_type.get_value());
        buf.put_varint(self.track_alias);
        buf.put_varint(self.group_id);
        if self.message_type.has_subgroup_id() {
            if let SubgroupId::Value(id) = self.subgroup_id {
                buf.put_varint(id);
            }
        }
        buf.put_u8(self.publisher_priority);
        buf
    }
}

#[derive(Debug, Clone)]
pub struct SubgroupObjectField {
    pub message_type: SubgroupHeaderType,
    pub object_id_delta: u64,
    pub extension_headers: ExtensionHeaders,
    pub subgroup_object: SubgroupObject,
}

impl SubgroupObjectField {
    pub fn is_end_of_group(&self) -> bool {
        self.message_type.has_end_of_group()
    }

    pub(crate) fn decode(
        message_type: SubgroupHeaderType,
        buf: &mut BytesMut,
    ) -> Result<Self, DecodeError> {
        let mut cursor = std::io::Cursor::<&[u8]>::new(&buf);
        let object_id_delta = cursor
            .try_get_varint()
            .log_context("Subgroup Object ID Delta")
            .map_err(|_| DecodeError::NeedMoreData)?;
        let extension_headers = if message_type.has_extensions() {
            ExtensionHeaders::decode(&mut cursor).ok_or(DecodeError::NeedMoreData)?
        } else {
            ExtensionHeaders {
                prior_group_id_gap: vec![],
                prior_object_id_gap: vec![],
                immutable_extensions: vec![],
            }
        };
        let length = SubgroupObject::check_length(&mut cursor).ok_or(DecodeError::NeedMoreData)?;
        let subgroup_object = if length == 0 {
            SubgroupObject::decode_status(&mut cursor).ok_or(DecodeError::NeedMoreData)?
        } else {
            buf.split_to(cursor.position() as usize);
            SubgroupObject::decode_payload(length, buf)
        };
        Ok(Self {
            message_type,
            object_id_delta,
            extension_headers,
            subgroup_object,
        })
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_varint(self.object_id_delta);
        if self.message_type.has_extensions() {
            let ext_headers_buf = self.extension_headers.encode();
            buf.extend_from_slice(&ext_headers_buf);
        }
        buf.extend_from_slice(&self.subgroup_object.encode());
        buf
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use bytes::Bytes;

        use crate::modules::moqt::data_plane::object::{
            extension_headers::ExtensionHeaders,
            subgroup::{
                SubgroupHeader, SubgroupHeaderType, SubgroupId, SubgroupObject, SubgroupObjectField,
            },
        };

        // --- SubgroupHeader Tests ---

        #[test]
        fn subgroup_header_packetize_and_depacketize_zero_id() {
            let header = SubgroupHeader::new(
                // No Subgroup ID field, No Extensions, No End of Group
                1,
                2,
                SubgroupId::None,
                128,
                false,
                false,
            );

            let buf = header.encode();
            let mut cursor = std::io::Cursor::<&[u8]>::new(&buf);
            let depacketized = SubgroupHeader::decode(&mut cursor).unwrap();

            assert_eq!(header.message_type, depacketized.message_type);
            assert_eq!(header.track_alias, depacketized.track_alias);
            assert_eq!(header.group_id, depacketized.group_id);
            assert!(matches!(depacketized.subgroup_id, SubgroupId::None));
            assert_eq!(header.publisher_priority, depacketized.publisher_priority);

            // Check raw bytes for 0x10
            // Message Type (0x10), Track Alias (1), Group ID (2), Publisher Priority (128)
            let expected_bytes = vec![0x10, 0x01, 0x02, 0x80];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }

        #[test]
        fn subgroup_header_packetize_and_depacketize_first_object_id_delta() {
            let header = SubgroupHeader::new(
                // First Object ID, No Extensions, No End of Group
                3,
                4,
                SubgroupId::FirstObjectIdDelta,
                64,
                false,
                false,
            );

            let buf = header.encode();
            let mut cursor = std::io::Cursor::<&[u8]>::new(&buf);
            let depacketized = SubgroupHeader::decode(&mut cursor).unwrap();

            assert_eq!(header.message_type, depacketized.message_type);
            assert_eq!(header.track_alias, depacketized.track_alias);
            assert_eq!(header.group_id, depacketized.group_id);
            assert!(matches!(
                depacketized.subgroup_id,
                SubgroupId::FirstObjectIdDelta
            ));
            assert_eq!(header.publisher_priority, depacketized.publisher_priority);

            // Check raw bytes for 0x12
            // Message Type (0x12), Track Alias (3), Group ID (4), Publisher Priority (64)
            let expected_bytes = vec![0x12, 0x03, 0x04, 0x40];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }

        #[test]
        fn subgroup_header_packetize_and_depacketize_value_id() {
            let header = SubgroupHeader::new(5, 6, SubgroupId::Value(100), 32, true, true);

            let buf = header.encode();
            let mut cursor = std::io::Cursor::<&[u8]>::new(&buf);
            let depacketized = SubgroupHeader::decode(&mut cursor).unwrap();

            assert_eq!(header.message_type, depacketized.message_type);
            assert_eq!(header.track_alias, depacketized.track_alias);
            assert_eq!(header.group_id, depacketized.group_id);
            if let SubgroupId::Value(v) = depacketized.subgroup_id {
                assert_eq!(v, 100);
            } else {
                panic!("Expected SubgroupId::Value(100)");
            }
            assert_eq!(header.publisher_priority, depacketized.publisher_priority);

            // Check raw bytes for 0x14
            // Message Type (0x14), Track Alias (5), Group ID (6), Subgroup ID (100), Publisher Priority (32)
            let expected_bytes = vec![0x1D, 0x05, 0x06, 0x40, 0x64, 0x20];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }

        // --- SubgroupObjectField Tests ---

        #[test]
        fn subgroup_object_field_packetize_and_depacketize_no_extensions() {
            let message_type = SubgroupHeaderType::new(0x10).unwrap();
            let payload = Bytes::from(vec![0xDE, 0xAD, 0xBE, 0xEF]);
            let object_field = SubgroupObjectField {
                message_type,
                object_id_delta: 1,
                extension_headers: ExtensionHeaders {
                    prior_group_id_gap: vec![],
                    prior_object_id_gap: vec![],
                    immutable_extensions: vec![],
                },
                subgroup_object: SubgroupObject::new_payload(payload.clone()),
            };

            let mut buf = object_field.encode();
            let depacketized = SubgroupObjectField::decode(message_type, &mut buf).unwrap();

            assert_eq!(object_field.object_id_delta, depacketized.object_id_delta);
            assert!(
                depacketized
                    .extension_headers
                    .immutable_extensions
                    .is_empty()
            );
            assert_eq!(object_field.subgroup_object, depacketized.subgroup_object);

            // Check raw bytes: Object ID Delta (1), Payload Length (4), Payload
            let expected_bytes = vec![0x01, 0x04, 0xDE, 0xAD, 0xBE, 0xEF];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }

        #[test]
        fn subgroup_object_field_packetize_and_depacketize_with_extensions() {
            let message_type = SubgroupHeaderType::new(0x11).unwrap();
            let payload = Bytes::from(vec![0x11, 0x22, 0x33]);
            let object_field = SubgroupObjectField {
                message_type,
                object_id_delta: 5,
                extension_headers: ExtensionHeaders {
                    prior_group_id_gap: vec![10],
                    prior_object_id_gap: vec![],
                    immutable_extensions: vec![Bytes::from(vec![0x01, 0x02])],
                },
                subgroup_object: SubgroupObject::new_payload(payload.clone()),
            };

            let mut buf = object_field.encode();
            let depacketized = SubgroupObjectField::decode(message_type, &mut buf).unwrap();

            assert_eq!(object_field.object_id_delta, depacketized.object_id_delta);
            assert_eq!(
                object_field.extension_headers.prior_group_id_gap,
                depacketized.extension_headers.prior_group_id_gap
            );
            assert_eq!(
                object_field.extension_headers.immutable_extensions,
                depacketized.extension_headers.immutable_extensions
            );
            assert_eq!(object_field.subgroup_object, depacketized.subgroup_object);

            // Expected bytes:
            // Object ID Delta (5) = 0x05
            // Extension Headers:
            //   Count: 2 (0x02)
            //   KV1: 3c 0a
            //   KV2: 0b 02 01 02
            // Payload Length: 3 (0x03)
            // Payload: 11 22 33
            let expected_bytes = vec![
                0x05, // object_id_delta
                0x02, // number of parameters
                0x3c, 0x0a, // KeyValuePair 1: Key=0x3c, Value=10
                0x0b, 0x02, 0x01,
                0x02, // KeyValuePair 2: Key=0x0b, ValueLen=2, Value=[0x01, 0x02]
                0x03, // payload length
                0x11, 0x22, 0x33, // object_payload
            ];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }

        #[test]
        fn subgroup_object_field_packetize_and_depacketize_status() {
            let message_type = SubgroupHeaderType::new(0x10).unwrap();
            let object_field = SubgroupObjectField {
                message_type,
                object_id_delta: 10,
                extension_headers: ExtensionHeaders {
                    prior_group_id_gap: vec![],
                    prior_object_id_gap: vec![],
                    immutable_extensions: vec![],
                },
                subgroup_object: SubgroupObject::new_status(3), // EndOfGroup
            };

            let mut buf = object_field.encode();
            let depacketized = SubgroupObjectField::decode(message_type, &mut buf).unwrap();

            assert_eq!(object_field.object_id_delta, depacketized.object_id_delta);
            assert!(
                depacketized
                    .extension_headers
                    .immutable_extensions
                    .is_empty()
            );
            assert_eq!(object_field.subgroup_object, depacketized.subgroup_object);

            // Check raw bytes: Object ID Delta (10=0x0A), Status Length (0), Status (3)
            let expected_bytes = vec![0x0A, 0x00, 0x03];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }

        #[test]
        fn subgroup_header_type_selection() {
            // Case 1: No Subgroup ID, No Extensions, No End of Group -> 0x10
            let h = SubgroupHeader::new(0, 0, SubgroupId::None, 0, false, false);
            let buf = h.encode();
            assert_eq!(buf[0], 0x10);

            // Case 2: First Object ID, Extensions, No End of Group -> 0x13
            // (Previously buggy logic would have returned 0x15)
            let h = SubgroupHeader::new(0, 0, SubgroupId::FirstObjectIdDelta, 0, true, false);
            let buf = h.encode();
            assert_eq!(buf[0], 0x13);

            // Case 3: Value, Extensions, End of Group -> 0x1D
            // (Previously buggy logic would have returned 0x1B)
            let h = SubgroupHeader::new(0, 0, SubgroupId::Value(100), 0, true, true);
            let buf = h.encode();
            assert_eq!(buf[0], 0x1D);
        }
    }
}
