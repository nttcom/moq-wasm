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
    moqt::data_plane::object::{key_value_pair::KeyValuePair, object_status::ObjectStatus},
};

#[repr(u64)]
enum DatagramTypeValue {
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

#[derive(Debug, Clone)]
pub(crate) enum DatagramField {
    Payload0x00 {
        object_id: u64,
        publisher_priority: u8,
        payload: Bytes,
    },
    Payload0x01 {
        object_id: u64,
        publisher_priority: u8,
        extension_headers: Vec<KeyValuePair>,
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
        extension_headers: Vec<KeyValuePair>,
        payload: Bytes,
    },
    Payload0x04 {
        object_id: u64,
        publisher_priority: u8,
        payload: Bytes,
    },
    Payload0x05 {
        publisher_priority: u8,
        extension_headers: Vec<KeyValuePair>,
        payload: Bytes,
    },
    Payload0x06WithEndOfGroup {
        publisher_priority: u8,
        payload: Bytes,
    },
    Payload0x07WithEndOfGroup {
        publisher_priority: u8,
        extension_headers: Vec<KeyValuePair>,
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
        extension_headers: Vec<KeyValuePair>,
        status: ObjectStatus,
    },
}

impl DatagramField {
    pub(crate) fn decode(message_type: u64, data: &mut BytesMut) -> Option<Self> {
        match message_type {
            val if val == DatagramTypeValue::Payload0x00 as u64 => {
                let object_id = data.try_get_varint().log_context("object id").ok()?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let payload = data.split_to(data.len());
                Some(Self::Payload0x00 {
                    object_id,
                    publisher_priority,
                    payload: payload.into(),
                })
            }
            val if val == DatagramTypeValue::Payload0x01 as u64 => {
                let object_id = data.try_get_varint().log_context("object id").ok()?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let extension_headers = Self::read_extension_headers(data)?;
                let payload = data.split_to(data.len());
                Some(Self::Payload0x01 {
                    object_id,
                    publisher_priority,
                    extension_headers,
                    payload: payload.into(),
                })
            }
            val if val == DatagramTypeValue::Payload0x02WithEndOfGroup as u64 => {
                Self::validate_end_of_group(data)?;
                let object_id = data.try_get_varint().log_context("object id").ok()?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let payload = data.split_to(data.len());
                Some(Self::Payload0x02WithEndOfGroup {
                    object_id,
                    publisher_priority,
                    payload: payload.into(),
                })
            }
            val if val == DatagramTypeValue::Payload0x03WithEndOfGroup as u64 => {
                Self::validate_end_of_group(data)?;
                let object_id = data.try_get_varint().log_context("object id").ok()?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let extension_headers = Self::read_extension_headers(data)?;
                let payload = data.split_to(data.len());
                Some(Self::Payload0x03WithEndOfGroup {
                    object_id,
                    publisher_priority,
                    extension_headers,
                    payload: payload.into(),
                })
            }
            val if val == DatagramTypeValue::Payload0x04 as u64 => {
                let object_id = data.try_get_varint().log_context("object id").ok()?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let payload = data.split_to(data.len());
                Some(Self::Payload0x04 {
                    object_id,
                    publisher_priority,
                    payload: payload.into(),
                })
            }
            val if val == DatagramTypeValue::Payload0x05 as u64 => {
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let extension_headers = Self::read_extension_headers(data)?;
                let payload = data.split_to(data.len());
                Some(Self::Payload0x05 {
                    publisher_priority,
                    extension_headers,
                    payload: payload.into(),
                })
            }
            val if val == DatagramTypeValue::Payload0x06WithEndOfGroup as u64 => {
                Self::validate_end_of_group(data)?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let payload = data.split_to(data.len());
                Some(Self::Payload0x06WithEndOfGroup {
                    publisher_priority,
                    payload: payload.into(),
                })
            }
            val if val == DatagramTypeValue::Payload0x07WithEndOfGroup as u64 => {
                Self::validate_end_of_group(data)?;
                let publisher_priority =
                    data.try_get_u8().log_context("publisher priority").ok()?;
                let extension_headers = Self::read_extension_headers(data)?;
                let payload = data.split_to(data.len());
                Some(Self::Payload0x07WithEndOfGroup {
                    publisher_priority,
                    extension_headers,
                    payload: payload.into(),
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
                let extension_headers = Self::read_extension_headers(data)?;
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
                buf.put_varint(extension_headers.len() as u64);
                for header in extension_headers {
                    let extension_header = header.encode();
                    buf.unsplit(extension_header);
                }
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
                buf.put_varint(extension_headers.len() as u64);
                for header in extension_headers {
                    let extension_header = header.encode();
                    buf.unsplit(extension_header);
                }
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
                buf.put_varint(extension_headers.len() as u64);
                for header in extension_headers {
                    let extension_header = header.encode();
                    buf.unsplit(extension_header);
                }
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
                buf.put_varint(extension_headers.len() as u64);
                for header in extension_headers {
                    let extension_header = header.encode();
                    buf.unsplit(extension_header);
                }
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
                for header in extension_headers {
                    let extension_header = header.encode();
                    buf.unsplit(extension_header);
                }
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
            Err(_) => return None,
        }
    }

    fn read_extension_headers(data: &mut BytesMut) -> Option<Vec<KeyValuePair>> {
        let mut extension_headers = Vec::new();
        let length = data
            .try_get_varint()
            .log_context("extension headers length")
            .ok()?;
        for _ in 0..length {
            let ext_header = KeyValuePair::decode(data)?;
            extension_headers.push(ext_header);
        }
        Some(extension_headers)
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use super::super::*;
        use crate::modules::moqt::data_plane::object::key_value_pair::{KeyValuePair, VariantType};
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
                assert_eq!(payload, Bytes::from(vec![1, 2, 3, 4, 5]));
            } else {
                panic!("Decoded into wrong variant");
            }
            assert_eq!(encoded.remaining(), 0);
        }

        #[test]
        fn payload0x01_encode_decode() {
            // setup
            let field = DatagramField::Payload0x01 {
                object_id: 456,
                publisher_priority: 20,
                extension_headers: vec![KeyValuePair {
                    key: 1,
                    value: VariantType::Odd(Bytes::from(vec![10, 20])),
                }],
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
                assert_eq!(extension_headers.len(), 1);
                assert_eq!(
                    extension_headers[0],
                    KeyValuePair {
                        key: 1,
                        value: VariantType::Odd(Bytes::from(vec![10, 20]))
                    }
                );
                assert_eq!(payload, Bytes::from(vec![6, 7, 8, 9, 10]));
            } else {
                panic!("Decoded into wrong variant");
            }
            assert_eq!(encoded.remaining(), 0);
        }
    }
}

