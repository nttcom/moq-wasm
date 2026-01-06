use anyhow::bail;
use bytes::BytesMut;

use crate::modules::moqt::control_plane::messages::variable_integer::{
    read_variable_integer_from_buffer, write_variable_integer,
};
use crate::modules::moqt::data_plane::object::key_value_pair::KeyValuePair;

type ExtensionHeader = KeyValuePair;

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

#[derive(Debug, Clone)]
pub enum SubgroupId {
    Zero,
    FirstObjectIdDelta,
    Value(u64),
}

#[derive(Debug, Clone)]
pub struct SubgroupHeader {
    pub message_type: u64,
    pub track_alias: u64,
    pub group_id: u64,
    pub subgroup_id: SubgroupId,
    pub publisher_priority: u8,
}

impl SubgroupHeader {
    pub(crate) fn depacketize(mut buf: BytesMut) -> anyhow::Result<Self> {
        let message_type = read_variable_integer_from_buffer(&mut buf)?;
        if !Self::validate_message_type(message_type) {
            bail!("Invalid message type: {}", message_type);
        }
        let track_alias = read_variable_integer_from_buffer(&mut buf)?;
        let group_id = read_variable_integer_from_buffer(&mut buf)?;
        let subgroup_id = Self::read_subgroup_id(message_type, &mut buf)?;
        let publisher_priority = u8::try_from(read_variable_integer_from_buffer(&mut buf)?)?;
        Ok(Self {
            message_type,
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        })
    }

    fn validate_message_type(message_type: u64) -> bool {
        (0x10..=0x1d).contains(&message_type)
    }

    fn read_subgroup_id(message_type: u64, bytes: &mut BytesMut) -> anyhow::Result<SubgroupId> {
        match message_type {
            0x10 | 0x11 | 0x18 | 0x19 => Ok(SubgroupId::Zero),
            0x12 | 0x13 | 0x1A | 0x1B => Ok(SubgroupId::FirstObjectIdDelta),
            0x14 | 0x15 | 0x1C | 0x1D => {
                let subgroup = read_variable_integer_from_buffer(bytes)?;
                Ok(SubgroupId::Value(subgroup))
            }
            _ => bail!("Invalid message type: {}", message_type),
        }
    }

    pub(crate) fn packetize(&self) -> anyhow::Result<BytesMut> {
        let mut buf = BytesMut::new();
        buf.extend(write_variable_integer(self.message_type));
        buf.extend(write_variable_integer(self.track_alias));
        buf.extend(write_variable_integer(self.group_id));
        if self.write_subgroup_id(&mut buf).is_ok() {
            buf.extend(write_variable_integer(self.publisher_priority as u64));
            Ok(buf)
        } else {
            bail!("Invalid message type: {}", self.message_type);
        }
    }

    fn write_subgroup_id(&self, bytes: &mut BytesMut) -> anyhow::Result<()> {
        match self.subgroup_id {
            SubgroupId::Zero => {
                if self.message_type == 0x10
                    || self.message_type == 0x11
                    || self.message_type == 0x18
                    || self.message_type == 0x19
                {
                    tracing::info!("Subgroup ID: Zero");
                    Ok(())
                } else {
                    bail!("Invalid message type: {}", self.message_type);
                }
            }
            SubgroupId::FirstObjectIdDelta => {
                if self.message_type == 0x12
                    || self.message_type == 0x13
                    || self.message_type == 0x1A
                    || self.message_type == 0x1B
                {
                    tracing::info!("Subgroup ID: FirstObjectIdDelta");
                    Ok(())
                } else {
                    bail!("Invalid message type: {}", self.message_type);
                }
            }
            SubgroupId::Value(v) => {
                if self.message_type == 0x14
                    || self.message_type == 0x15
                    || self.message_type == 0x1C
                    || self.message_type == 0x1D
                {
                    tracing::info!("Subgroup ID: Value");
                    bytes.extend(write_variable_integer(v));
                    Ok(())
                } else {
                    bail!("Invalid message type: {}", self.message_type);
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubgroupObjectField {
    pub object_id_delta: u64,
    pub extension_headers: Vec<ExtensionHeader>,
    pub object_payload: Vec<u8>,
}

impl SubgroupObjectField {
    pub(crate) fn depacketize(mut buf: BytesMut) -> anyhow::Result<Self> {
        let object_id_delta = read_variable_integer_from_buffer(&mut buf)?;
        let extension_headers_total_len = read_variable_integer_from_buffer(&mut buf)?;
        let mut extension_headers_bytes = buf.split_to(extension_headers_total_len as usize);
        let mut extension_headers = Vec::new();
        while !extension_headers_bytes.is_empty() {
            let header = ExtensionHeader::decode(&mut extension_headers_bytes)
                .ok_or_else(|| anyhow::anyhow!("Failed to decode extension header"))?;
            extension_headers.push(header);
        }
        // The remaining bytes in `buf` are the object payload
        // The original code had `buf.split_to(buf.len())` which is correct for the payload.
        let object_payload = buf.split_to(buf.len());
        Ok(Self {
            object_id_delta,
            extension_headers,
            object_payload: object_payload.to_vec(),
        })
    }

    pub(crate) fn packetize(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.extend(write_variable_integer(self.object_id_delta));

        let mut headers_buf = BytesMut::new();
        for header in &self.extension_headers {
            let header_bytes = header.encode();
            headers_buf.extend_from_slice(&header_bytes);
        }
        buf.extend(write_variable_integer(headers_buf.len() as u64)); // Write total byte length
        buf.extend(headers_buf); // Write serialized headers

        buf.extend_from_slice(&self.object_payload);
        buf
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use bytes::Bytes;

        use crate::modules::moqt::data_plane::object::{
            key_value_pair::{KeyValuePair, VariantType},
            subgroup::{SubgroupHeader, SubgroupId, SubgroupObjectField},
        };

        // --- SubgroupHeader Tests ---

        #[test]
        fn subgroup_header_packetize_and_depacketize_zero_id() {
            let header = SubgroupHeader {
                message_type: 0x10, // No Subgroup ID field, No Extensions, No End of Group
                track_alias: 1,
                group_id: 2,
                subgroup_id: SubgroupId::Zero,
                publisher_priority: 128,
            };

            let buf = header.packetize().unwrap();
            let depacketized = SubgroupHeader::depacketize(buf.clone()).unwrap();

            assert_eq!(header.message_type, depacketized.message_type);
            assert_eq!(header.track_alias, depacketized.track_alias);
            assert_eq!(header.group_id, depacketized.group_id);
            assert!(matches!(depacketized.subgroup_id, SubgroupId::Zero));
            assert_eq!(header.publisher_priority, depacketized.publisher_priority);

            // Check raw bytes for 0x10
            // Message Type (0x10), Track Alias (1), Group ID (2), Publisher Priority (128)
            let expected_bytes = vec![0x10, 0x01, 0x02, 0x40, 0x80];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }

        #[test]
        fn subgroup_header_packetize_and_depacketize_first_object_id_delta() {
            let header = SubgroupHeader {
                message_type: 0x12, // First Object ID, No Extensions, No End of Group
                track_alias: 3,
                group_id: 4,
                subgroup_id: SubgroupId::FirstObjectIdDelta,
                publisher_priority: 64,
            };

            let buf = header.packetize().unwrap();
            let depacketized = SubgroupHeader::depacketize(buf.clone()).unwrap();

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
            let expected_bytes = vec![0x12, 0x03, 0x04, 0x40, 0x40];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }

        #[test]
        fn subgroup_header_packetize_and_depacketize_value_id() {
            let header = SubgroupHeader {
                message_type: 0x14, // Subgroup ID field present, No Extensions, No End of Group
                track_alias: 5,
                group_id: 6,
                subgroup_id: SubgroupId::Value(100),
                publisher_priority: 32,
            };

            let buf = header.packetize().unwrap();
            let depacketized = SubgroupHeader::depacketize(buf.clone()).unwrap();

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
            let expected_bytes = vec![0x14, 0x05, 0x06, 0x40, 0x64, 0x20];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }

        // --- SubgroupObjectField Tests ---

        #[test]
        fn subgroup_object_field_packetize_and_depacketize_no_extensions() {
            let object_field = SubgroupObjectField {
                object_id_delta: 1,
                extension_headers: vec![],
                object_payload: vec![0xDE, 0xAD, 0xBE, 0xEF],
            };

            let buf = object_field.packetize();
            let depacketized = SubgroupObjectField::depacketize(buf.clone()).unwrap();

            assert_eq!(object_field.object_id_delta, depacketized.object_id_delta);
            assert!(depacketized.extension_headers.is_empty());
            assert_eq!(object_field.object_payload, depacketized.object_payload);

            // Check raw bytes: Object ID Delta (1), Extension Headers Length (0), Payload
            let expected_bytes = vec![0x01, 0x00, 0xDE, 0xAD, 0xBE, 0xEF];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }

        #[test]
        fn subgroup_object_field_packetize_and_depacketize_with_extensions() {
            let object_field = SubgroupObjectField {
                object_id_delta: 5,
                extension_headers: vec![
                    KeyValuePair {
                        key: 0x3c, // PriorGroupIdGap
                        value: VariantType::Even(10),
                    },
                    KeyValuePair {
                        key: 0x0b, // ImmutableExtensions
                        value: VariantType::Odd(Bytes::from(vec![0x01, 0x02])),
                    },
                ],
                object_payload: vec![0x11, 0x22, 0x33],
            };

            let buf = object_field.packetize();
            let depacketized = SubgroupObjectField::depacketize(buf.clone()).unwrap();

            assert_eq!(object_field.object_id_delta, depacketized.object_id_delta);
            assert_eq!(
                object_field.extension_headers.len(),
                depacketized.extension_headers.len()
            );
            assert_eq!(
                object_field.extension_headers[0].key,
                depacketized.extension_headers[0].key
            );
            assert!(matches!(
                depacketized.extension_headers[0].value,
                VariantType::Even(10)
            ));
            assert_eq!(
                object_field.extension_headers[1].key,
                depacketized.extension_headers[1].key
            );
            if let VariantType::Odd(v) = &depacketized.extension_headers[1].value {
                assert_eq!(v, &vec![0x01, 0x02]);
            } else {
                panic!("Expected VariantType::Odd");
            }
            assert_eq!(object_field.object_payload, depacketized.object_payload);

            // Expected bytes:
            // Object ID Delta (5) = 0x05
            // Extension Headers:
            //   - KeyValuePair 1: Key (0x3c), Value (10) => 0x3c 0x0a
            //   - KeyValuePair 2: Key (0x0b), Value Length (2), Value (0x01 0x02) => 0x0b 0x02 0x01 0x02
            // Total Extension Headers Length = 2 + 4 = 6 bytes
            // Payload = 0x11 0x22 0x33
            let expected_bytes = vec![
                0x05, // object_id_delta
                0x06, // extension_headers_total_len (6 bytes)
                0x3c, 0x0a, // KeyValuePair 1: Key=0x3c, Value=10
                0x0b, 0x02, 0x01,
                0x02, // KeyValuePair 2: Key=0x0b, ValueLen=2, Value=[0x01, 0x02]
                0x11, 0x22, 0x33, // object_payload
            ];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }
    }
}
