use bytes::{Buf, Bytes, BytesMut};

use crate::modules::extensions::{
    buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariantType {
    Odd(Bytes),
    Even(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValuePair {
    pub key: u64,
    pub value: VariantType,
}

impl KeyValuePair {
    pub(crate) fn decode(bytes: &mut impl Buf) -> Option<Self> {
        let key = bytes
            .try_get_varint()
            .log_context("KeyValuePair key")
            .ok()?;
        if key % 2 == 0 {
            let value = bytes
                .try_get_varint()
                .log_context("KeyValuePair value")
                .ok()?;
            Some(Self {
                key,
                value: VariantType::Even(value),
            })
        } else {
            let length = bytes
                .try_get_varint()
                .log_context("KeyValuePair length")
                .ok()?;
            let value = bytes.copy_to_bytes(length as usize);
            Some(Self {
                key,
                value: VariantType::Odd(value),
            })
        }
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_varint(self.key);
        match &self.value {
            VariantType::Odd(value) => {
                buf.put_varint(value.len() as u64);
                buf.extend_from_slice(value);
            }
            VariantType::Even(value) => {
                buf.put_varint(*value);
            }
        }
        buf
    }
}

#[cfg(test)]
mod tests {
    mod success {

        use crate::modules::moqt::control_plane::control_messages::key_value_pair::{
            KeyValuePair, VariantType,
        };
        use bytes::Bytes;

        #[test]
        fn packetize_and_depacketize_even_key() {
            let kv_pair = KeyValuePair {
                key: 0x3c, // PriorGroupIdGap (even)
                value: VariantType::Even(10),
            };

            let buf = kv_pair.encode();
            let mut buf = std::io::Cursor::new(&buf[..]);
            let depacketized = KeyValuePair::decode(&mut buf).unwrap();

            assert_eq!(kv_pair.key, depacketized.key);
            match depacketized.value {
                VariantType::Even(v) => assert_eq!(v, 10),
                _ => panic!("Expected VariantType::Even"),
            }
        }

        #[test]
        fn packetize_and_depacketize_odd_key() {
            let kv_pair = KeyValuePair {
                key: 0x0b, // ImmutableExtensions (odd)
                value: VariantType::Odd(Bytes::from(vec![0x01, 0x02, 0x03])),
            };

            let buf = kv_pair.encode();
            let mut buf = std::io::Cursor::new(&buf[..]);
            let depacketized = KeyValuePair::decode(&mut buf).unwrap();

            assert_eq!(kv_pair.key, depacketized.key);
            match depacketized.value {
                VariantType::Odd(v) => assert_eq!(v, vec![0x01, 0x02, 0x03]),
                _ => panic!("Expected VariantType::Odd"),
            }
        }

        #[test]
        fn packetize_check_bytes_even_key() {
            let kv_pair = KeyValuePair {
                key: 0x3c,
                value: VariantType::Even(10),
            };

            let buf = kv_pair.encode();

            // key (0x3c) + value (10)
            let expected_bytes = vec![0x3c, 0x0a];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }

        #[test]
        fn packetize_check_bytes_odd_key() {
            let kv_pair = KeyValuePair {
                key: 0x0b,
                value: VariantType::Odd(Bytes::from(vec![0x01, 0x02, 0x03])),
            };

            let buf = kv_pair.encode();

            // key (0x0b) + value length (3) + value ([0x01, 0x02, 0x03])
            let expected_bytes = vec![0x0b, 0x03, 0x01, 0x02, 0x03];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }
    }
}
