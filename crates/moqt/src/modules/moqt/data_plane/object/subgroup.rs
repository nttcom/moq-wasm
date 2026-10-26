use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::modules::extensions::buf_get_ext::BufGetExt;
use crate::modules::extensions::buf_put_ext::BufPutExt;
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

impl SubgroupId {
    // TODO: FirstObjectIdDelta should use the first object's object_id as subgroup_id
    pub fn resolve(&self) -> u64 {
        match self {
            SubgroupId::None | SubgroupId::FirstObjectIdDelta => 0,
            SubgroupId::Value(id) => *id,
        }
    }
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
                let subgroup_id = buf.try_get_varint()?;
                Ok(SubgroupId::Value(subgroup_id))
            }
            _ => Err(DecodeError::Fatal(format!(
                "Invalid message type: {}",
                self.0
            ))),
        }
    }

    fn has_subgroup_id(&self) -> bool {
        matches!(self.0, 0x14 | 0x15 | 0x1C | 0x1D)
    }

    fn has_extensions(&self) -> bool {
        matches!(self.0, 0x11 | 0x13 | 0x15 | 0x19 | 0x1B | 0x1D)
    }

    pub fn has_end_of_group(&self) -> bool {
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

    pub fn decode(cursor: &mut std::io::Cursor<&[u8]>) -> Result<Self, DecodeError> {
        let message_type = cursor.try_get_varint()?;
        let message_type = SubgroupHeaderType::new(message_type)
            .ok_or_else(|| DecodeError::Fatal("Invalid message type".to_string()))?;
        let track_alias = cursor.try_get_varint()?;
        let group_id = cursor.try_get_varint()?;
        let subgroup_id = message_type.get_subgroup_id_field(cursor)?;
        let publisher_priority = cursor.try_get_u8()?;
        Ok(Self {
            message_type,
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        })
    }

    pub fn encode(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_varint(self.message_type.get_value());
        buf.put_varint(self.track_alias);
        buf.put_varint(self.group_id);
        if self.message_type.has_subgroup_id()
            && let SubgroupId::Value(id) = self.subgroup_id
        {
            buf.put_varint(id);
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
    pub fn resolve_object_id(&self, prev: Option<u64>) -> u64 {
        object_id_from_delta(self.object_id_delta, prev)
    }

    pub fn encode(&self) -> BytesMut {
        let (head, payload) = match &self.subgroup_object {
            SubgroupObject::Payload { data, .. } => (
                SubgroupObjectHead::payload(
                    self.object_id_delta,
                    self.extension_headers.clone(),
                    data.len(),
                ),
                &data[..],
            ),
            SubgroupObject::Status { code, .. } => (
                SubgroupObjectHead::status(
                    self.object_id_delta,
                    self.extension_headers.clone(),
                    *code,
                ),
                &[][..],
            ),
        };
        let mut buf = head.encode(self.message_type);
        buf.extend_from_slice(payload);
        buf
    }
}

fn object_id_from_delta(object_id_delta: u64, prev: Option<u64>) -> u64 {
    match prev {
        None => object_id_delta,
        Some(prev) => prev + 1 + object_id_delta,
    }
}

/// The fields of a subgroup object that precede its payload (draft-14
/// §10.4.2). A status object ends with them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SubgroupObjectHead {
    pub(crate) object_id_delta: u64,
    pub(crate) extension_headers: ExtensionHeaders,
    body: ObjectBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectBody {
    /// `length` (non-zero) payload bytes follow the head.
    Payload { length: usize },
    /// A zero Object Payload Length followed by the status.
    Status { code: u64 },
}

impl SubgroupObjectHead {
    /// `length` must not be zero: a zero length announces a status.
    pub(crate) fn payload(
        object_id_delta: u64,
        extension_headers: ExtensionHeaders,
        length: usize,
    ) -> Self {
        Self {
            object_id_delta,
            extension_headers,
            body: ObjectBody::Payload { length },
        }
    }

    pub(crate) fn status(
        object_id_delta: u64,
        extension_headers: ExtensionHeaders,
        code: u64,
    ) -> Self {
        Self {
            object_id_delta,
            extension_headers,
            body: ObjectBody::Status { code },
        }
    }

    /// Reads the head from `cursor`, leaving it right before the payload.
    pub(crate) fn decode(
        message_type: SubgroupHeaderType,
        cursor: &mut std::io::Cursor<&[u8]>,
    ) -> Result<Self, DecodeError> {
        let object_id_delta = cursor.try_get_varint()?;
        let extension_headers = if message_type.has_extensions() {
            ExtensionHeaders::decode(cursor).ok_or(DecodeError::NeedMoreData)?
        } else {
            ExtensionHeaders::default()
        };
        let body = match cursor.try_get_varint()? as usize {
            0 => ObjectBody::Status {
                code: cursor.try_get_varint()?,
            },
            length => ObjectBody::Payload { length },
        };
        Ok(Self {
            object_id_delta,
            extension_headers,
            body,
        })
    }

    pub(crate) fn encode(&self, message_type: SubgroupHeaderType) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_varint(self.object_id_delta);
        if message_type.has_extensions() {
            buf.extend_from_slice(&self.extension_headers.encode());
        }
        match self.body {
            ObjectBody::Payload { length } => buf.put_varint(length as u64),
            ObjectBody::Status { code } => {
                buf.put_varint(0);
                buf.put_varint(code);
            }
        }
        buf
    }

    pub(crate) fn resolve_object_id(&self, prev: Option<u64>) -> u64 {
        object_id_from_delta(self.object_id_delta, prev)
    }

    pub(crate) fn payload_len(&self) -> usize {
        match self.body {
            ObjectBody::Payload { length } => length,
            ObjectBody::Status { .. } => 0,
        }
    }

    /// `payload` must be `payload_len()` bytes long.
    pub(crate) fn with_payload(
        self,
        message_type: SubgroupHeaderType,
        payload: Bytes,
    ) -> SubgroupObjectField {
        let subgroup_object = match self.body {
            ObjectBody::Payload { .. } => SubgroupObject::new_payload(payload),
            ObjectBody::Status { code } => SubgroupObject::new_status(code),
        };
        SubgroupObjectField {
            message_type,
            object_id_delta: self.object_id_delta,
            extension_headers: self.extension_headers,
            subgroup_object,
        }
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use bytes::Bytes;

        use crate::modules::moqt::data_plane::object::{
            extension_headers::ExtensionHeaders,
            subgroup::{
                SubgroupHeader, SubgroupHeaderType, SubgroupId, SubgroupObject,
                SubgroupObjectField, SubgroupObjectHead,
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

        /// Decodes an encoded object the way the subgroup decoder does: the
        /// head, then the payload after it.
        fn decode_field(message_type: SubgroupHeaderType, buf: &[u8]) -> SubgroupObjectField {
            let mut cursor = std::io::Cursor::new(buf);
            let head = SubgroupObjectHead::decode(message_type, &mut cursor).unwrap();
            let payload = Bytes::copy_from_slice(&buf[cursor.position() as usize..]);
            assert_eq!(payload.len(), head.payload_len());
            head.with_payload(message_type, payload)
        }

        #[test]
        fn subgroup_object_field_packetize_and_depacketize_no_extensions() {
            let message_type = SubgroupHeaderType::new(0x10).unwrap();
            let payload = Bytes::from(vec![0xDE, 0xAD, 0xBE, 0xEF]);
            let object_field = SubgroupObjectField {
                message_type,
                object_id_delta: 1,
                extension_headers: ExtensionHeaders::default(),
                subgroup_object: SubgroupObject::new_payload(payload.clone()),
            };

            let buf = object_field.encode();
            let depacketized = decode_field(message_type, &buf);

            assert_eq!(object_field.object_id_delta, depacketized.object_id_delta);
            assert!(
                depacketized
                    .extension_headers
                    .immutable_extensions()
                    .is_empty()
            );
            assert_eq!(object_field.subgroup_object, depacketized.subgroup_object);
        }

        #[test]
        fn subgroup_object_field_packetize_and_depacketize_with_extensions() {
            let message_type = SubgroupHeaderType::new(0x11).unwrap();
            let payload = Bytes::from(vec![0x11, 0x22, 0x33]);
            let mut extension_headers = ExtensionHeaders::default();
            extension_headers.push_prior_group_id_gap(10);
            extension_headers.push_immutable_extension(Bytes::from(vec![0x01, 0x02]));
            let object_field = SubgroupObjectField {
                message_type,
                object_id_delta: 5,
                extension_headers,
                subgroup_object: SubgroupObject::new_payload(payload.clone()),
            };

            let buf = object_field.encode();
            let depacketized = decode_field(message_type, &buf);

            assert_eq!(object_field.object_id_delta, depacketized.object_id_delta);
            assert_eq!(
                object_field.extension_headers.prior_group_id_gap(),
                depacketized.extension_headers.prior_group_id_gap()
            );
            assert_eq!(
                object_field.extension_headers.immutable_extensions(),
                depacketized.extension_headers.immutable_extensions()
            );
            assert_eq!(object_field.subgroup_object, depacketized.subgroup_object);
        }

        #[test]
        fn subgroup_object_field_packetize_and_depacketize_status() {
            let message_type = SubgroupHeaderType::new(0x10).unwrap();
            let object_field = SubgroupObjectField {
                message_type,
                object_id_delta: 10,
                extension_headers: ExtensionHeaders::default(),
                subgroup_object: SubgroupObject::new_status(3), // EndOfGroup
            };

            let buf = object_field.encode();
            let depacketized = decode_field(message_type, &buf);

            assert_eq!(object_field.object_id_delta, depacketized.object_id_delta);
            assert!(
                depacketized
                    .extension_headers
                    .immutable_extensions()
                    .is_empty()
            );
            assert_eq!(object_field.subgroup_object, depacketized.subgroup_object);
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

        // --- resolve_object_id Tests ---

        fn make_field(delta: u64) -> SubgroupObjectField {
            SubgroupObjectField {
                message_type: SubgroupHeaderType::new(0x10).unwrap(),
                object_id_delta: delta,
                extension_headers: ExtensionHeaders::default(),
                subgroup_object: SubgroupObject::new_payload(bytes::Bytes::new()),
            }
        }

        // --- resolve_object_id Tests ---

        #[test]
        fn resolve_object_id_first_object() {
            // First object: delta IS the object_id
            assert_eq!(make_field(0).resolve_object_id(None), 0);
            assert_eq!(make_field(5).resolve_object_id(None), 5);
        }

        #[test]
        fn resolve_object_id_sequential() {
            // Sequential 0,1,2,3: all deltas are 0
            let mut prev = None;
            for expected in 0..4 {
                let id = make_field(0).resolve_object_id(prev);
                assert_eq!(id, expected);
                prev = Some(id);
            }
        }

        #[test]
        fn resolve_object_id_with_gap() {
            // 0, 5, 10: deltas are 0, 4, 4
            let id0 = make_field(0).resolve_object_id(None);
            assert_eq!(id0, 0);
            let id1 = make_field(4).resolve_object_id(Some(id0));
            assert_eq!(id1, 5);
            let id2 = make_field(4).resolve_object_id(Some(id1));
            assert_eq!(id2, 10);
        }
    }

    mod failure {
        use super::super::*;

        #[test]
        fn object_head_decode_split_payload_length_varint_needs_more_data() {
            // Arrange
            let header = SubgroupHeader::new(1, 2, SubgroupId::None, 128, false, false);
            let buf = [0u8, 0x40];

            // Act
            let result = SubgroupObjectHead::decode(
                header.message_type,
                &mut std::io::Cursor::new(&buf[..]),
            );

            // Assert
            assert!(matches!(result, Err(DecodeError::NeedMoreData)));
        }
    }
}
