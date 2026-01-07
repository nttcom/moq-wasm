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

use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::data_plane::object::{extension_header::ExtensionHeader, object_status::ObjectStatus},
};

#[repr(u64)]
pub(crate) enum DatagramTypeValue {
    Payload0x00 = 0x00,
    Payload0x01 = 0x01,
    Payload0x02WithEndOfGroup = 0x02,
    Payload0x03WithEndOfGroup = 0x03,
    Payload0x04 = 0x04,
    Payload0x05 = 0x05,
    Payload0x06WithEndOfGroup = 0x06,
    Payload0x07WithEndOfGroup = 0x07,
    Status0x20 = 0x20,
    Status0x21 = 0x21,
}

impl DatagramTypeValue {
    pub(crate) fn from_datagram_field(val: &DatagramField) -> Self {
        match val {
            DatagramField::Payload0x00 { .. } => Self::Payload0x00,
            DatagramField::Payload0x01 { .. } => Self::Payload0x01,
            DatagramField::Payload0x02WithEndOfGroup { .. } => Self::Payload0x02WithEndOfGroup,
            DatagramField::Payload0x03WithEndOfGroup { .. } => Self::Payload0x03WithEndOfGroup,
            DatagramField::Payload0x04 { .. } => Self::Payload0x04,
            DatagramField::Payload0x05 { .. } => Self::Payload0x05,
            DatagramField::Payload0x06WithEndOfGroup { .. } => Self::Payload0x06WithEndOfGroup,
            DatagramField::Payload0x07WithEndOfGroup { .. } => Self::Payload0x07WithEndOfGroup,
            DatagramField::Status0x20 { .. } => Self::Status0x20,
            DatagramField::Status0x21 { .. } => Self::Status0x21,
        }
    }
}

#[derive(Debug, Clone)]
pub enum DatagramField {
    Payload0x00 {
        object_id: u64,
        publisher_priority: u8,
        payload: Bytes,
    },
    Payload0x01 {
        object_id: u64,
        publisher_priority: u8,
        extension_headers: ExtensionHeader,
        payload: Bytes,
    },
    Payload0x02WithEndOfGroup {
        object_id: u64,
        publisher_priority: u8,
        payload: Bytes,
    },
    Payload0x03WithEndOfGroup {
        object_id: u64,
        publisher_priority: u8,
        extension_headers: ExtensionHeader,
        payload: Bytes,
    },
    Payload0x04 {
        object_id: u64,
        publisher_priority: u8,
        payload: Bytes,
    },
    Payload0x05 {
        publisher_priority: u8,
        extension_headers: ExtensionHeader,
        payload: Bytes,
    },
    Payload0x06WithEndOfGroup {
        publisher_priority: u8,
        payload: Bytes,
    },
    Payload0x07WithEndOfGroup {
        publisher_priority: u8,
        extension_headers: ExtensionHeader,
        payload: Bytes,
    },
    Status0x20 {
        object_id: u64,
        publisher_priority: u8,
        status: ObjectStatus,
    },
    Status0x21 {
        object_id: u64,
        publisher_priority: u8,
        extension_headers: ExtensionHeader,
        status: ObjectStatus,
    },
}

impl DatagramField {
    pub fn to_bytes(data: impl Into<Bytes>) -> Bytes {
        data.into()
    }

    pub fn object_id(&self) -> Option<u64> {
        match self {
            Self::Payload0x00 { object_id, .. }
            | Self::Payload0x01 { object_id, .. }
            | Self::Payload0x02WithEndOfGroup { object_id, .. }
            | Self::Payload0x03WithEndOfGroup { object_id, .. }
            | Self::Payload0x04 { object_id, .. }
            | Self::Status0x20 { object_id, .. }
            | Self::Status0x21 { object_id, .. } => Some(*object_id),
            _ => None,
        }
    }

    pub(crate) fn decode(message_type: u64, data: &mut BytesMut) -> Option<Self> {
        match message_type {
            val if val == DatagramTypeValue::Payload0x00 as u64 => {
                let object_id = data.try_get_varint().log_context("object id").ok()?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let payload = data.clone().freeze();
                data.advance(payload.len());
                Some(Self::Payload0x00 {
                    object_id,
                    publisher_priority,
                    payload,
                })
            }
            val if val == DatagramTypeValue::Payload0x01 as u64 => {
                let object_id = data.try_get_varint().log_context("object id").ok()?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let extension_headers = ExtensionHeader::decode(data)?;
                let payload = data.clone().freeze();
                data.advance(payload.len());
                Some(Self::Payload0x01 {
                    object_id,
                    publisher_priority,
                    extension_headers,
                    payload,
                })
            }
            val if val == DatagramTypeValue::Payload0x02WithEndOfGroup as u64 => {
                Self::validate_end_of_group(data)?;
                let object_id = data.try_get_varint().log_context("object id").ok()?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let payload = data.clone().freeze();
                data.advance(payload.len());
                Some(Self::Payload0x02WithEndOfGroup {
                    object_id,
                    publisher_priority,
                    payload,
                })
            }
            val if val == DatagramTypeValue::Payload0x03WithEndOfGroup as u64 => {
                Self::validate_end_of_group(data)?;
                let object_id = data.try_get_varint().log_context("object id").ok()?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let extension_headers = ExtensionHeader::decode(data)?;
                let payload = data.clone().freeze();
                data.advance(payload.len());
                Some(Self::Payload0x03WithEndOfGroup {
                    object_id,
                    publisher_priority,
                    extension_headers,
                    payload,
                })
            }
            val if val == DatagramTypeValue::Payload0x04 as u64 => {
                let object_id = data.try_get_varint().log_context("object id").ok()?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let payload = data.clone().freeze();
                data.advance(payload.len());
                Some(Self::Payload0x04 {
                    object_id,
                    publisher_priority,
                    payload,
                })
            }
            val if val == DatagramTypeValue::Payload0x05 as u64 => {
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let extension_headers = ExtensionHeader::decode(data)?;
                let payload = data.clone().freeze();
                data.advance(payload.len());
                Some(Self::Payload0x05 {
                    publisher_priority,
                    extension_headers,
                    payload,
                })
            }
            val if val == DatagramTypeValue::Payload0x06WithEndOfGroup as u64 => {
                Self::validate_end_of_group(data)?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let payload = data.clone().freeze();
                data.advance(payload.len());
                Some(Self::Payload0x06WithEndOfGroup {
                    publisher_priority,
                    payload,
                })
            }
            val if val == DatagramTypeValue::Payload0x07WithEndOfGroup as u64 => {
                Self::validate_end_of_group(data)?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let extension_headers = ExtensionHeader::decode(data)?;
                let payload = data.clone().freeze();
                data.advance(payload.len());
                Some(Self::Payload0x07WithEndOfGroup {
                    publisher_priority,
                    extension_headers,
                    payload,
                })
            }
            val if val == DatagramTypeValue::Status0x20 as u64 => {
                let object_id = data.try_get_varint().log_context("object id").ok()?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let status = data.try_get_u8().log_context("status").ok()?;
                Some(Self::Status0x20 {
                    object_id,
                    publisher_priority,
                    status: ObjectStatus::try_from(status).ok()?,
                })
            }
            val if val == DatagramTypeValue::Status0x21 as u64 => {
                let object_id = data.try_get_varint().log_context("object id").ok()?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let extension_headers = ExtensionHeader::decode(data)?;
                let status = data.try_get_u8().log_context("status").ok()?;
                Some(Self::Status0x21 {
                    object_id,
                    publisher_priority,
                    extension_headers,
                    status: ObjectStatus::try_from(status).ok()?,
                })
            }
            _ => {
                tracing::error!("Invalid message type: {}", message_type);
                None
            }
        }
    }
    pub(crate) fn encode(&self) -> (u64, BytesMut) {
        let mut buf = BytesMut::new();
        match self {
            Self::Payload0x00 {
                object_id,
                publisher_priority,
                payload,
            } => {
                buf.put_varint(*object_id);
                buf.put_u8(*publisher_priority);
                buf.extend_from_slice(payload);
                (DatagramTypeValue::Payload0x00 as u64, buf)
            }
            Self::Payload0x01 {
                object_id,
                publisher_priority,
                extension_headers,
                payload,
            } => {
                buf.put_varint(*object_id);
                buf.put_u8(*publisher_priority);
                buf.unsplit(extension_headers.encode());
                buf.extend_from_slice(payload);
                (DatagramTypeValue::Payload0x01 as u64, buf)
            }
            Self::Payload0x02WithEndOfGroup {
                object_id,
                publisher_priority,
                payload,
            } => {
                buf.put_u8(1);
                buf.put_varint(*object_id);
                buf.put_u8(*publisher_priority);
                buf.extend_from_slice(payload);
                (DatagramTypeValue::Payload0x02WithEndOfGroup as u64, buf)
            }
            Self::Payload0x03WithEndOfGroup {
                object_id,
                publisher_priority,
                extension_headers,
                payload,
            } => {
                buf.put_u8(1);
                buf.put_varint(*object_id);
                buf.put_u8(*publisher_priority);
                buf.unsplit(extension_headers.encode());
                buf.extend_from_slice(payload);
                (DatagramTypeValue::Payload0x03WithEndOfGroup as u64, buf)
            }
            Self::Payload0x04 {
                object_id,
                publisher_priority,
                payload,
            } => {
                buf.put_varint(*object_id);
                buf.put_u8(*publisher_priority);
                buf.extend_from_slice(payload);
                (DatagramTypeValue::Payload0x04 as u64, buf)
            }
            Self::Payload0x05 {
                publisher_priority,
                extension_headers,
                payload,
            } => {
                buf.put_u8(*publisher_priority);
                buf.unsplit(extension_headers.encode());
                buf.extend_from_slice(payload);
                (DatagramTypeValue::Payload0x05 as u64, buf)
            }
            Self::Payload0x06WithEndOfGroup {
                publisher_priority,
                payload,
            } => {
                buf.put_u8(1);
                buf.put_u8(*publisher_priority);
                buf.extend_from_slice(payload);
                (DatagramTypeValue::Payload0x06WithEndOfGroup as u64, buf)
            }
            Self::Payload0x07WithEndOfGroup {
                publisher_priority,
                extension_headers,
                payload,
            } => {
                buf.put_u8(1);
                buf.put_u8(*publisher_priority);
                buf.unsplit(extension_headers.encode());
                buf.extend_from_slice(payload);
                (DatagramTypeValue::Payload0x07WithEndOfGroup as u64, buf)
            }
            Self::Status0x20 {
                object_id,
                publisher_priority,
                status,
            } => {
                buf.put_varint(*object_id);
                buf.put_u8(*publisher_priority);
                buf.put_u8(*status as u8);
                (DatagramTypeValue::Status0x20 as u64, buf)
            }
            Self::Status0x21 {
                object_id,
                publisher_priority,
                extension_headers,
                status,
            } => {
                buf.put_varint(*object_id);
                buf.put_u8(*publisher_priority);
                buf.unsplit(extension_headers.encode());
                buf.put_u8(*status as u8);
                (DatagramTypeValue::Status0x21 as u64, buf)
            }
        }
    }

    fn validate_end_of_group(data: &mut BytesMut) -> Option<()> {
        match data.try_get_u8().log_context("end of group") {
            Ok(val) => {
                if val != 1 {
                    return None;
                }
                Some(())
            }
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    mod success {

        use std::vec;

        use super::super::*;

        use bytes::{Buf, Bytes};

        #[test]
        fn payload0x00_encode_decode() {
            // setup
            let field = DatagramField::Payload0x00 {
                object_id: 123,
                publisher_priority: 10,
                payload: Bytes::from(vec![1, 2, 3, 4, 5]),
            };

            // execution
            let (message_type, mut encoded) = field.encode();
            let decoded = DatagramField::decode(message_type, &mut encoded).unwrap();

            // validation
            assert_eq!(message_type, 0x00);
            if let DatagramField::Payload0x00 {
                object_id,
                publisher_priority,
                payload,
            } = decoded
            {
                assert_eq!(object_id, 123);
                assert_eq!(publisher_priority, 10);
                assert_eq!(*payload, vec![1, 2, 3, 4, 5]);
            } else {
                panic!("Decoded into wrong variant");
            }
            assert_eq!(encoded.remaining(), 0);
        }

        #[test]
        fn payload0x01_encode_decode() {
            // setup
            let extension_header = ExtensionHeader {
                prior_group_id_gap: vec![],
                prior_object_id_gap: vec![],
                immutable_extensions: vec![Bytes::from(vec![10, 20])],
            };
            let field = DatagramField::Payload0x01 {
                object_id: 456,
                publisher_priority: 20,
                extension_headers: extension_header,
                payload: Bytes::from(vec![6, 7, 8, 9, 10]),
            };

            // execution
            let (message_type, mut encoded) = field.encode();
            let decoded = DatagramField::decode(message_type, &mut encoded).unwrap();

            // validation
            assert_eq!(message_type, 0x01);
            if let DatagramField::Payload0x01 {
                object_id,
                publisher_priority,
                extension_headers,
                payload,
            } = decoded
            {
                assert_eq!(object_id, 456);
                assert_eq!(publisher_priority, 20);
                assert_eq!(
                    extension_headers.immutable_extensions,
                    vec![Bytes::from(vec![10, 20])]
                );
                assert_eq!(*payload, Bytes::from(vec![6, 7, 8, 9, 10]));
            } else {
                panic!("Decoded into wrong variant");
            }
            assert_eq!(encoded.remaining(), 0);
        }

        #[test]
        fn payload0x02_with_end_of_group_encode_decode() {
            // setup
            let field = DatagramField::Payload0x02WithEndOfGroup {
                object_id: 789,
                publisher_priority: 30,
                payload: Bytes::from(vec![11, 12, 13, 14, 15]),
            };

            // execution
            let (message_type, mut encoded) = field.encode();
            let decoded = DatagramField::decode(message_type, &mut encoded).unwrap();

            // validation
            assert_eq!(message_type, 0x02);
            if let DatagramField::Payload0x02WithEndOfGroup {
                object_id,
                publisher_priority,
                payload,
            } = decoded
            {
                assert_eq!(object_id, 789);
                assert_eq!(publisher_priority, 30);
                assert_eq!(*payload, vec![11, 12, 13, 14, 15]);
            } else {
                panic!("Decoded into wrong variant");
            }
            assert_eq!(encoded.remaining(), 0);
        }

        #[test]
        fn payload0x03_with_end_of_group_encode_decode() {
            // setup
            let extension_header = ExtensionHeader {
                prior_group_id_gap: vec![],
                prior_object_id_gap: vec![],
                immutable_extensions: vec![],
            };
            let field = DatagramField::Payload0x03WithEndOfGroup {
                object_id: 101,
                publisher_priority: 40,
                extension_headers: extension_header,
                payload: Bytes::from(vec![16, 17, 18, 19, 20]),
            };

            // execution
            let (message_type, mut encoded) = field.encode();
            let decoded = DatagramField::decode(message_type, &mut encoded).unwrap();

            // validation
            assert_eq!(message_type, 0x03);
            if let DatagramField::Payload0x03WithEndOfGroup {
                object_id,
                publisher_priority,
                extension_headers,
                payload,
            } = decoded
            {
                assert_eq!(object_id, 101);
                assert_eq!(publisher_priority, 40);
                assert_eq!(
                    extension_headers,
                    ExtensionHeader {
                        prior_group_id_gap: vec![],
                        prior_object_id_gap: vec![],
                        immutable_extensions: vec![]
                    }
                );
                assert_eq!(*payload, Bytes::from(vec![16, 17, 18, 19, 20]));
            } else {
                panic!("Decoded into wrong variant");
            }
            assert_eq!(encoded.remaining(), 0);
        }

        #[test]
        fn payload0x04_encode_decode() {
            // setup
            let field = DatagramField::Payload0x04 {
                object_id: 112,
                publisher_priority: 50,
                payload: Bytes::from(vec![21, 22, 23, 24, 25]),
            };

            // execution
            let (message_type, mut encoded) = field.encode();
            let decoded = DatagramField::decode(message_type, &mut encoded).unwrap();

            // validation
            assert_eq!(message_type, 0x04);
            if let DatagramField::Payload0x04 {
                object_id,
                publisher_priority,
                payload,
            } = decoded
            {
                assert_eq!(object_id, 112);
                assert_eq!(publisher_priority, 50);
                assert_eq!(*payload, vec![21, 22, 23, 24, 25]);
            } else {
                panic!("Decoded into wrong variant");
            }
            assert_eq!(encoded.remaining(), 0);
        }

        #[test]
        fn payload0x05_encode_decode() {
            // setup
            let extension_header = ExtensionHeader {
                prior_group_id_gap: vec![3],
                prior_object_id_gap: vec![],
                immutable_extensions: vec![],
            };
            let field = DatagramField::Payload0x05 {
                publisher_priority: 60,
                extension_headers: extension_header,
                payload: Bytes::from(vec![26, 27, 28, 29, 30]),
            };

            // execution
            let (message_type, mut encoded) = field.encode();
            let decoded = DatagramField::decode(message_type, &mut encoded).unwrap();

            // validation
            assert_eq!(message_type, 0x05);
            if let DatagramField::Payload0x05 {
                publisher_priority,
                extension_headers,
                payload,
            } = decoded
            {
                assert_eq!(publisher_priority, 60);
                assert_eq!(
                    extension_headers,
                    ExtensionHeader {
                        prior_group_id_gap: vec![3],
                        prior_object_id_gap: vec![],
                        immutable_extensions: vec![]
                    }
                );
                assert_eq!(*payload, vec![26, 27, 28, 29, 30]);
            } else {
                panic!("Decoded into wrong variant");
            }
            assert_eq!(encoded.remaining(), 0);
        }

        #[test]
        fn payload0x06_with_end_of_group_encode_decode() {
            // setup
            let field = DatagramField::Payload0x06WithEndOfGroup {
                publisher_priority: 70,
                payload: Bytes::from(vec![31, 32, 33, 34, 35]),
            };

            // execution
            let (message_type, mut encoded) = field.encode();
            let decoded = DatagramField::decode(message_type, &mut encoded).unwrap();

            // validation
            assert_eq!(message_type, 0x06);
            if let DatagramField::Payload0x06WithEndOfGroup {
                publisher_priority,
                payload,
            } = decoded
            {
                assert_eq!(publisher_priority, 70);
                assert_eq!(*payload, vec![31, 32, 33, 34, 35]);
            } else {
                panic!("Decoded into wrong variant");
            }
            assert_eq!(encoded.remaining(), 0);
        }

        #[test]
        fn payload0x07_with_end_of_group_encode_decode() {
            // setup
            let extension_header = ExtensionHeader {
                prior_group_id_gap: vec![],
                prior_object_id_gap: vec![3],
                immutable_extensions: vec![],
            };
            let field = DatagramField::Payload0x07WithEndOfGroup {
                publisher_priority: 80,
                extension_headers: extension_header,
                payload: Bytes::from(vec![36, 37, 38, 39, 40]),
            };

            // execution
            let (message_type, mut encoded) = field.encode();
            let decoded = DatagramField::decode(message_type, &mut encoded).unwrap();

            // validation
            assert_eq!(message_type, 0x07);
            if let DatagramField::Payload0x07WithEndOfGroup {
                publisher_priority,
                extension_headers,
                payload,
            } = decoded
            {
                assert_eq!(publisher_priority, 80);
                assert_eq!(
                    extension_headers,
                    ExtensionHeader {
                        prior_group_id_gap: vec![],
                        prior_object_id_gap: vec![3],
                        immutable_extensions: vec![]
                    }
                );
                assert_eq!(*payload, vec![36, 37, 38, 39, 40]);
            } else {
                panic!("Decoded into wrong variant");
            }
            assert_eq!(encoded.remaining(), 0);
        }

        #[test]
        fn status0x20_encode_decode() {
            // setup
            let field = DatagramField::Status0x20 {
                object_id: 131,
                publisher_priority: 90,
                status: ObjectStatus::DoesNotExist,
            };

            // execution
            let (message_type, mut encoded) = field.encode();
            let decoded = DatagramField::decode(message_type, &mut encoded).unwrap();

            // validation
            assert_eq!(message_type, 0x20);
            if let DatagramField::Status0x20 {
                object_id,
                publisher_priority,
                status,
            } = decoded
            {
                assert_eq!(object_id, 131);
                assert_eq!(publisher_priority, 90);
                assert_eq!(status, ObjectStatus::DoesNotExist);
            } else {
                panic!("Decoded into wrong variant");
            }
            assert_eq!(encoded.remaining(), 0);
        }

        #[test]
        fn status0x21_encode_decode() {
            // setup
            let extension_header = ExtensionHeader {
                prior_group_id_gap: vec![],
                prior_object_id_gap: vec![],
                immutable_extensions: vec![],
            };
            let field = DatagramField::Status0x21 {
                object_id: 141,
                publisher_priority: 100,
                extension_headers: extension_header,
                status: ObjectStatus::EndOfGroup,
            };

            // execution
            let (message_type, mut encoded) = field.encode();
            let decoded = DatagramField::decode(message_type, &mut encoded).unwrap();

            // validation
            assert_eq!(message_type, 0x21);
            if let DatagramField::Status0x21 {
                object_id,
                publisher_priority,
                extension_headers,
                status,
            } = decoded
            {
                assert_eq!(object_id, 141);
                assert_eq!(publisher_priority, 100);
                assert_eq!(
                    extension_headers,
                    ExtensionHeader {
                        prior_group_id_gap: vec![],
                        prior_object_id_gap: vec![],
                        immutable_extensions: vec![]
                    }
                );
                assert_eq!(status, ObjectStatus::EndOfGroup);
            } else {
                panic!("Decoded into wrong variant");
            }
            assert_eq!(encoded.remaining(), 0);
        }
    }
}
