use anyhow::Context;
use bytes::{Buf, Bytes, BytesMut};

use crate::modules::extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt};

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
    pub(crate) fn decode(bytes: &mut impl Buf) -> anyhow::Result<Self> {
        let key = bytes.try_get_varint().context("KeyValuePair key")?;
        if key % 2 == 0 {
            let value = bytes.try_get_varint().context("KeyValuePair value")?;
            Ok(Self {
                key,
                value: VariantType::Even(value),
            })
        } else {
            let length = bytes.try_get_varint().context("KeyValuePair length")?;
            // copy_to_bytes panics when the buffer is shorter than the
            // peer-controlled length, so reject truncated input first.
            if bytes.remaining() < length as usize {
                anyhow::bail!(
                    "KeyValuePair value truncated: length {} > remaining {}",
                    length,
                    bytes.remaining()
                );
            }
            let value = bytes.copy_to_bytes(length as usize);
            Ok(Self {
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

    pub(crate) fn decode_list(bytes: &mut impl Buf) -> anyhow::Result<Vec<Self>> {
        let length = bytes.try_get_varint().context("number of parameters")?;
        (0..length).map(|_| Self::decode(bytes)).collect()
    }

    pub(crate) fn encode_list(pairs: &[Self]) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_varint(pairs.len() as u64);
        pairs.iter().for_each(|pair| buf.unsplit(pair.encode()));
        buf
    }

    pub(crate) fn even(key: u64, value: u64) -> Self {
        Self {
            key,
            value: VariantType::Even(value),
        }
    }

    pub(crate) fn odd(key: u64, value: Bytes) -> Self {
        Self {
            key,
            value: VariantType::Odd(value),
        }
    }
}

pub(crate) fn find_even(pairs: &[KeyValuePair], key: u64) -> Option<u64> {
    pairs
        .iter()
        .find(|pair| pair.key == key)
        .and_then(|pair| match pair.value {
            VariantType::Even(value) => Some(value),
            VariantType::Odd(_) => None,
        })
}

pub(crate) fn odd_values(pairs: &[KeyValuePair], key: u64) -> impl Iterator<Item = &Bytes> {
    pairs
        .iter()
        .filter(move |pair| pair.key == key)
        .filter_map(|pair| match &pair.value {
            VariantType::Odd(value) => Some(value),
            VariantType::Even(_) => None,
        })
}

#[cfg(test)]
mod tests {
    mod success {

        use crate::modules::moqt::control_plane::control_messages::key_value_pair::{
            KeyValuePair, VariantType, find_even, odd_values,
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

        #[test]
        fn list_round_trips_and_finds_values_by_key() {
            // Arrange
            let pairs = vec![
                KeyValuePair::odd(0x03, Bytes::from_static(b"a")),
                KeyValuePair::even(0x02, 10),
                KeyValuePair::odd(0x03, Bytes::from_static(b"b")),
            ];

            // Act
            let buf = KeyValuePair::encode_list(&pairs);
            let decoded = KeyValuePair::decode_list(&mut std::io::Cursor::new(&buf[..])).unwrap();

            // Assert
            assert_eq!(decoded, pairs);
            assert_eq!(find_even(&decoded, 0x02), Some(10));
            assert_eq!(find_even(&decoded, 0x04), None);
            assert_eq!(
                odd_values(&decoded, 0x03).collect::<Vec<_>>(),
                [&Bytes::from_static(b"a"), &Bytes::from_static(b"b")]
            );
        }
    }

    mod failure {
        use crate::modules::moqt::control_plane::control_messages::key_value_pair::KeyValuePair;

        #[test]
        fn depacketize_odd_key_value_truncated() {
            // key (0x0b, odd) + value length (3) but only 1 byte of value
            let bytes = [0x0b_u8, 0x03, 0x01];
            let mut buf = std::io::Cursor::new(&bytes[..]);

            assert!(KeyValuePair::decode(&mut buf).is_err());
        }

        #[test]
        fn depacketize_odd_key_length_without_value() {
            // key (0x0b, odd) + value length (3) and nothing else
            let bytes = [0x0b_u8, 0x03];
            let mut buf = std::io::Cursor::new(&bytes[..]);

            assert!(KeyValuePair::decode(&mut buf).is_err());
        }

        #[test]
        fn depacketize_even_key_without_value() {
            // key (0x3c, even) and no value varint
            let bytes = [0x3c_u8];
            let mut buf = std::io::Cursor::new(&bytes[..]);

            assert!(KeyValuePair::decode(&mut buf).is_err());
        }
    }
}
