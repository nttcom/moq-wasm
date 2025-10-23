use anyhow::bail;
use bytes::BytesMut;

use crate::modules::moqt::messages::{
    object::key_value_pair::KeyValuePair,
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

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

pub enum SubgroupId {
    Zero,
    FirstObjectIdDelta,
    Value(u64),
}

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

pub struct SubgroupObjectField {
    pub object_id_delta: u64,
    pub extension_headers: Vec<ExtensionHeader>,
    pub object_payload: Vec<u8>,
}

impl SubgroupObjectField {
    pub(crate) fn depacketize(mut buf: BytesMut) -> anyhow::Result<Self> {
        let object_id_delta = read_variable_integer_from_buffer(&mut buf)?;
        let extension_headers_length = read_variable_integer_from_buffer(&mut buf)?;
        for _ in 0..extension_headers_length {
            ExtensionHeader::depacketize(&mut buf)?;
        }
        let object_payload = buf.split_to(buf.len());
        Ok(Self {
            object_id_delta,
            extension_headers: vec![],
            object_payload: object_payload.to_vec(),
        })
    }

    pub(crate) fn packetize(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.extend(write_variable_integer(self.object_id_delta));
        buf.extend(write_variable_integer(self.extension_headers.len() as u64));
        for header in &self.extension_headers {
            let header = header.packetize();
            buf.extend_from_slice(&header);
        }
        buf.extend_from_slice(&self.object_payload);
        buf
    }
}
