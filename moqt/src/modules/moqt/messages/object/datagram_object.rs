use anyhow::bail;
use bytes::BytesMut;

use crate::modules::moqt::messages::{
    object::{key_value_pair::KeyValuePair, object_status::ObjectStatus},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

type ExtensionHeader = KeyValuePair;

// +======+==============+============+===========+==================+
// | Type | End Of Group | Extensions | Object ID | Status / Payload |
// +======+==============+============+===========+==================+
// |      |              | Present    | Present   |                  |
// +------+--------------+------------+-----------+------------------+
// | 0x00 | No           | No         | Yes       | Payload          |
// +------+--------------+------------+-----------+------------------+
// | 0x01 | No           | Yes        | Yes       | Payload          |
// +------+--------------+------------+-----------+------------------+
// | 0x02 | Yes          | No         | Yes       | Payload          |
// +------+--------------+------------+-----------+------------------+
// | 0x03 | Yes          | Yes        | Yes       | Payload          |
// +------+--------------+------------+-----------+------------------+
// | 0x04 | No           | No         | No        | Payload          |
// +------+--------------+------------+-----------+------------------+
// | 0x05 | No           | Yes        | No        | Payload          |
// +------+--------------+------------+-----------+------------------+
// | 0x06 | Yes          | No         | No        | Payload          |
// +------+--------------+------------+-----------+------------------+
// | 0x07 | Yes          | Yes        | No        | Payload          |
// +------+--------------+------------+-----------+------------------+
// | 0x20 | No           | No         | Yes       | Status           |
// +------+--------------+------------+-----------+------------------+
// | 0x21 | No           | Yes        | Yes       | Status           |
// +------+--------------+------------+-----------+------------------+

pub struct DatagramObject {
    pub(crate) message_type: u64,
    pub(crate) track_alias: u64,
    pub(crate) group_id: u64,
    pub(crate) object_id: u64,
    pub(crate) publisher_priority: u8,
    pub(crate) extension_headers: Vec<ExtensionHeader>,
    pub(crate) object_status: Option<ObjectStatus>,
    pub(crate) object_payload: Vec<u8>,
}

impl DatagramObject {
    pub(crate) fn depacketize(buf: &mut BytesMut) -> anyhow::Result<Self> {
        let message_type = read_variable_integer_from_buffer(buf)?;
        if !Self::validate_message_type(message_type) {
            bail!("Invalid message type: {}", message_type);
        }
        let track_alias = read_variable_integer_from_buffer(buf)?;
        let group_id = read_variable_integer_from_buffer(buf)?;
        let object_id = Self::read_object_id(message_type, buf)?;
        let publisher_priority = u8::try_from(read_variable_integer_from_buffer(buf)?)?;
        let extension_headers = Self::read_extension_headers(message_type, buf)?;
        let object_status = Self::read_object_status(message_type, buf)?;
        let object_payload = buf.split_to(buf.len()).to_vec();
        Ok(Self {
            message_type,
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            extension_headers,
            object_status,
            object_payload,
        })
    }

    fn validate_message_type(message_type: u64) -> bool {
        (0x00..=0x07).contains(&message_type)
            || message_type == 0x20 || message_type == 0x21
    }

    fn read_object_id(message_type: u64, bytes: &mut BytesMut) -> anyhow::Result<u64> {
        if message_type == 0x00
            || message_type == 0x01
            || message_type == 0x02
            || message_type == 0x03
            || message_type == 0x20
            || message_type == 0x21
        {
            Ok(read_variable_integer_from_buffer(bytes)?)
        } else {
            Ok(0)
        }
    }

    fn read_extension_headers(
        message_type: u64,
        bytes: &mut BytesMut,
    ) -> anyhow::Result<Vec<ExtensionHeader>> {
        if message_type % 2 == 0 {
            Ok(Vec::new())
        } else {
            let extension_headers_length = read_variable_integer_from_buffer(bytes)?;
            let mut extension_headers_bytes = bytes.split_to(extension_headers_length as usize);
            let mut extension_headers = Vec::new();
            for _ in 0..extension_headers_length {
                let result = ExtensionHeader::depacketize(&mut extension_headers_bytes)?;
                extension_headers.push(result);
            }
            Ok(extension_headers)
        }
    }

    fn read_object_status(
        message_type: u64,
        bytes: &mut BytesMut,
    ) -> anyhow::Result<Option<ObjectStatus>> {
        if message_type == 0x20 || message_type == 0x21 {
            let status = u8::try_from(read_variable_integer_from_buffer(bytes)?)?;
            Ok(Some(ObjectStatus::try_from(status)?))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn packetize(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.extend(write_variable_integer(self.message_type));
        buf.extend(write_variable_integer(self.track_alias));
        buf.extend(write_variable_integer(self.group_id));
        if self.object_id > 0 {
            buf.extend(write_variable_integer(self.object_id));
        }
        buf.extend(write_variable_integer(self.publisher_priority as u64));
        self.write_extension_headers(&mut buf);
        self.write_object_status_or_payload(&mut buf);

        buf
    }

    fn write_extension_headers(&self, bytes: &mut BytesMut) {
        if self.message_type % 2 != 0 {
            bytes.extend(write_variable_integer(self.extension_headers.len() as u64));
            for header in &self.extension_headers {
                let header = header.packetize();
                bytes.extend_from_slice(&header);
            }
        }
    }

    fn write_object_status_or_payload(&self, bytes: &mut BytesMut) {
        if self.message_type == 0x20 || self.message_type == 0x21 {
            bytes.extend(write_variable_integer(self.object_status.unwrap() as u64));
        } else {
            bytes.extend_from_slice(&self.object_payload);
        }
    }
}
