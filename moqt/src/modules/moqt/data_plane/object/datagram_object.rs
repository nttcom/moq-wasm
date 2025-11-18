use anyhow::bail;
use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::modules::moqt::control_plane::messages::variable_integer::{
    read_variable_integer_from_buffer, write_variable_integer,
};
use crate::modules::moqt::data_plane::object::{
    key_value_pair::KeyValuePair, object_status::ObjectStatus,
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

#[derive(Debug, Clone)]
pub struct DatagramObject {
    pub(crate) message_type: u64,
    pub track_alias: u64,
    pub(crate) group_id: u64,
    pub(crate) object_id: u64,
    pub(crate) publisher_priority: u8,
    pub(crate) extension_headers: Vec<ExtensionHeader>,
    pub(crate) object_status: Option<ObjectStatus>,
    pub object_payload: Bytes,
}

impl DatagramObject {
    pub(crate) fn depacketize(buf: &mut BytesMut) -> anyhow::Result<Self> {
        tracing::warn!("qqq {:?}", buf);
        let message_type = read_variable_integer_from_buffer(buf)?;
        tracing::warn!("qqq message_type: {:?}", message_type);
        if !Self::validate_message_type(message_type) {
            bail!("Invalid message type: {}", message_type);
        }
        let track_alias = read_variable_integer_from_buffer(buf)?;
        tracing::warn!("qqq track_alias: {:?}", track_alias);
        let group_id = read_variable_integer_from_buffer(buf)?;
        tracing::warn!("qqq group_id: {:?}", group_id);
        let object_id = Self::read_object_id(message_type, buf)?;
        tracing::warn!("qqq object_id: {:?}", object_id);
        let publisher_priority = buf.get_u8();
        tracing::warn!("qqq publisher_priority: {:?}", publisher_priority);
        let extension_headers = Self::read_extension_headers(message_type, buf)?;
        tracing::warn!("qqq extension_headers: {:?}", extension_headers);
        let object_status = Self::read_object_status(message_type, buf)?;
        tracing::warn!("qqq object_status: {:?}", object_status);
        let object_payload = buf.split_to(buf.len()).to_vec();
        tracing::warn!("qqq object_payload: {:?}", object_payload);
        Ok(Self {
            message_type,
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            extension_headers,
            object_status,
            object_payload: Bytes::from(object_payload),
        })
    }

    fn validate_message_type(message_type: u64) -> bool {
        (0x00..=0x07).contains(&message_type) || message_type == 0x20 || message_type == 0x21
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
            let mut extension_headers_bytes = bytes.split_to(extension_headers_length as usize); // consume the bytes
            let mut extension_headers = Vec::new();
            while !extension_headers_bytes.is_empty() {
                let result = ExtensionHeader::decode(&mut extension_headers_bytes)
                    .ok_or_else(|| anyhow::anyhow!("failed to decode extension header"))?; // this will consume bytes from extension_headers_bytes
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
        if self.message_type <= 0x03 || self.message_type == 0x20 || self.message_type == 0x21 {
            buf.extend(write_variable_integer(self.object_id));
        }
        buf.put_u8(self.publisher_priority);
        self.write_extension_headers(&mut buf);
        self.write_object_status_or_payload(&mut buf);
        buf
    }

    fn write_extension_headers(&self, bytes: &mut BytesMut) {
        if self.message_type % 2 != 0 {
            let mut headers_buf = BytesMut::new();
            for header in &self.extension_headers {
                let header = header.encode();
                headers_buf.extend_from_slice(&header);
            }
            bytes.extend(write_variable_integer(headers_buf.len() as u64));
            bytes.extend(headers_buf);
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

#[cfg(test)]
mod tests {
    mod success {

        use bytes::Bytes;

        use crate::modules::moqt::data_plane::object::{
            datagram_object::DatagramObject,
            key_value_pair::{KeyValuePair, VariantType},
            object_status::ObjectStatus,
        };

        #[test]
        fn packetize_and_depacketize_payload_with_object_id_no_extensions() {
            let object = DatagramObject {
                message_type: 0x00,
                track_alias: 1,
                group_id: 2,
                object_id: 3,
                publisher_priority: 128,
                extension_headers: vec![],
                object_status: None,
                object_payload: Bytes::from(vec![0, 1, 2, 3]),
            };

            let mut buf = object.packetize();
            let depacketized = DatagramObject::depacketize(&mut buf).unwrap();

            assert_eq!(object.message_type, depacketized.message_type);
            assert_eq!(object.track_alias, depacketized.track_alias);
            assert_eq!(object.group_id, depacketized.group_id);
            assert_eq!(object.object_id, depacketized.object_id);
            assert_eq!(object.publisher_priority, depacketized.publisher_priority);
            assert!(depacketized.extension_headers.is_empty());
            assert!(depacketized.object_status.is_none());
            assert_eq!(object.object_payload, depacketized.object_payload);
        }

        #[test]
        fn packetize_and_depacketize_payload_with_object_id_with_extensions() {
            let object = DatagramObject {
                message_type: 0x01,
                track_alias: 1,
                group_id: 2,
                object_id: 3,
                publisher_priority: 128,
                extension_headers: vec![KeyValuePair {
                    key: 0x3c, // PriorGroupIdGap
                    value: VariantType::Even(5),
                }],
                object_status: None,
                object_payload: Bytes::from(vec![0, 1, 2, 3]),
            };

            let mut buf = object.packetize();
            let depacketized = DatagramObject::depacketize(&mut buf).unwrap();

            assert_eq!(object.message_type, depacketized.message_type);
            assert_eq!(object.track_alias, depacketized.track_alias);
            assert_eq!(object.group_id, depacketized.group_id);
            assert_eq!(object.object_id, depacketized.object_id);
            assert_eq!(object.publisher_priority, depacketized.publisher_priority);
            assert_eq!(
                object.extension_headers[0].key,
                depacketized.extension_headers[0].key
            );
            assert!(matches!(
                depacketized.extension_headers[0].value,
                VariantType::Even(5)
            ));
            assert!(depacketized.object_status.is_none());
            assert_eq!(object.object_payload, depacketized.object_payload);
        }

        #[test]
        fn packetize_and_depacketize_status_with_object_id_no_extensions() {
            let object = DatagramObject {
                message_type: 0x20,
                track_alias: 1,
                group_id: 2,
                object_id: 3,
                publisher_priority: 128,
                extension_headers: vec![],
                object_status: Some(ObjectStatus::DoesNotExist),
                object_payload: Bytes::from(vec![]),
            };

            let mut buf = object.packetize();
            let depacketized = DatagramObject::depacketize(&mut buf).unwrap();

            assert_eq!(object.message_type, depacketized.message_type);
            assert_eq!(object.track_alias, depacketized.track_alias);
            assert_eq!(object.group_id, depacketized.group_id);
            assert_eq!(object.object_id, depacketized.object_id);
            assert_eq!(object.publisher_priority, depacketized.publisher_priority);
            assert!(depacketized.extension_headers.is_empty());
            assert_eq!(object.object_status, depacketized.object_status);
            assert!(depacketized.object_payload.is_empty());
        }
    }
}
