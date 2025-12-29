use bytes::BytesMut;

use crate::modules::extensions::buf_get_ext::BufGetExt;
use crate::modules::extensions::buf_put_ext::BufPutExt;
use crate::modules::extensions::result_ext::ResultExt;
use crate::modules::moqt::data_plane::object::datagram_field::{DatagramField, DatagramTypeValue};
use crate::modules::moqt::data_plane::object::key_value_pair::KeyValuePair;

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
pub struct ObjectDatagram {
    pub(crate) _message_type: u64,
    pub track_alias: u64,
    pub group_id: u64,
    pub field: DatagramField,
}

impl ObjectDatagram {
    pub fn new(track_alias: u64, group_id: u64, field: DatagramField) -> Self {
        Self {
            _message_type: DatagramTypeValue::from_datagram_field(&field) as u64,
            track_alias,
            group_id,
            field,
        }
    }

    pub(crate) fn decode(buf: &mut BytesMut) -> Option<Self> {
        let message_type = buf.try_get_varint().log_context("datagram type").ok()?;
        let track_alias = buf.try_get_varint().log_context("track alias").ok()?;
        let group_id = buf.try_get_varint().log_context("group id").ok()?;
        let field = DatagramField::decode(message_type, buf)?;

        Some(Self {
            _message_type: message_type,
            track_alias,
            group_id,
            field,
        })
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        let (message_type, field_bytes) = self.field.encode();
        buf.put_varint(message_type);
        buf.put_varint(self.track_alias);
        buf.put_varint(self.group_id);
        buf.unsplit(field_bytes);

        buf
    }
}

#[cfg(test)]
mod tests {
    mod success {

        use bytes::Bytes;

        use crate::modules::moqt::data_plane::object::{
            datagram_field::DatagramField,
            key_value_pair::{KeyValuePair, VariantType},
            object_datagram::ObjectDatagram,
            object_status::ObjectStatus,
        };

        #[test]
        fn packetize_and_depacketize_payload_with_object_id_no_extensions() {
            let object = ObjectDatagram {
                _message_type: 0x00,
                track_alias: 1,
                group_id: 2,
                field: DatagramField::Payload0x00 {
                    object_id: 3,
                    publisher_priority: 128,
                    payload: Bytes::from(vec![0, 1, 2, 3]),
                },
            };

            let mut buf = object.encode();
            let depacketized = ObjectDatagram::decode(&mut buf).unwrap();

            assert_eq!(object.track_alias, depacketized.track_alias);
            assert_eq!(object.group_id, depacketized.group_id);
            assert_eq!(object._message_type, depacketized._message_type);

            let (
                DatagramField::Payload0x00 {
                    object_id,
                    publisher_priority,
                    payload,
                },
                DatagramField::Payload0x00 {
                    object_id: depacketized_object_id,
                    publisher_priority: depacketized_publisher_priority,
                    payload: depacketized_payload,
                },
            ) = (&object.field, &depacketized.field)
            else {
                panic!()
            };

            assert_eq!(object_id, depacketized_object_id);
            assert_eq!(publisher_priority, depacketized_publisher_priority);
            assert_eq!(payload, depacketized_payload);
        }

        #[test]
        fn packetize_and_depacketize_payload_with_object_id_with_extensions() {
            let object = ObjectDatagram {
                _message_type: 0x01,
                track_alias: 1,
                group_id: 2,
                field: DatagramField::Payload0x01 {
                    object_id: 3,
                    publisher_priority: 128,
                    extension_headers: vec![KeyValuePair {
                        key: 0x3c, // PriorGroupIdGap
                        value: VariantType::Even(5),
                    }],
                    payload: Bytes::from(vec![0, 1, 2, 3]),
                },
            };

            let mut buf = object.encode();
            let depacketized = ObjectDatagram::decode(&mut buf).unwrap();

            assert_eq!(object.track_alias, depacketized.track_alias);
            assert_eq!(object.group_id, depacketized.group_id);
            assert_eq!(object._message_type, depacketized._message_type);

            let (
                DatagramField::Payload0x01 {
                    object_id,
                    publisher_priority,
                    extension_headers,
                    payload,
                },
                DatagramField::Payload0x01 {
                    object_id: depacketized_object_id,
                    publisher_priority: depacketized_publisher_priority,
                    extension_headers: depacketized_extension_headers,
                    payload: depacketized_payload,
                },
            ) = (&object.field, &depacketized.field)
            else {
                panic!()
            };

            assert_eq!(object_id, depacketized_object_id);
            assert_eq!(publisher_priority, depacketized_publisher_priority);
            assert_eq!(
                extension_headers[0].key,
                depacketized_extension_headers[0].key
            );
            assert!(matches!(
                depacketized_extension_headers[0].value,
                VariantType::Even(5)
            ));
            assert_eq!(payload, depacketized_payload);
        }

        #[test]
        fn packetize_and_depacketize_status_with_object_id_no_extensions() {
            let object = ObjectDatagram {
                _message_type: 0x20,
                track_alias: 1,
                group_id: 2,
                field: DatagramField::Status0x20 {
                    object_id: 3,
                    publisher_priority: 128,
                    status: ObjectStatus::DoesNotExist,
                },
            };

            let mut buf = object.encode();
            let depacketized = ObjectDatagram::decode(&mut buf).unwrap();

            assert_eq!(object.track_alias, depacketized.track_alias);
            assert_eq!(object.group_id, depacketized.group_id);
            assert_eq!(object._message_type, depacketized._message_type);

            let (
                DatagramField::Status0x20 {
                    object_id,
                    publisher_priority,
                    status,
                },
                DatagramField::Status0x20 {
                    object_id: depacketized_object_id,
                    publisher_priority: depacketized_publisher_priority,
                    status: depacketized_status,
                },
            ) = (&object.field, &depacketized.field)
            else {
                panic!()
            };

            assert_eq!(object_id, depacketized_object_id);
            assert_eq!(publisher_priority, depacketized_publisher_priority);
            assert_eq!(status, depacketized_status);
        }
    }
}
