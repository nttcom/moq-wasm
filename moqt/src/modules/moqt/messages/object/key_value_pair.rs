use bytes::BytesMut;

use crate::modules::moqt::messages::{
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

pub enum VariantType {
    Odd(Vec<u8>),
    Even(u64),
}

pub struct KeyValuePair {
    pub key: u64,
    pub value: VariantType,
}

impl KeyValuePair {
    pub(crate) fn depacketize(bytes: &mut BytesMut) -> anyhow::Result<Self> {
        let key = read_variable_integer_from_buffer(bytes)?;
        if key % 2 == 0 {
            let value = read_variable_integer_from_buffer(bytes)?;
            Ok(Self {
                key,
                value: VariantType::Even(value),
            })
        } else {
            let value = read_variable_bytes_from_buffer(bytes)?;
            Ok(Self {
                key,
                value: VariantType::Odd(value),
            })
        }
    }

    pub(crate) fn packetize(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.extend(write_variable_integer(self.key));
        match &self.value {
            VariantType::Odd(value) => {
                buf.extend(write_variable_bytes(value));
            }
            VariantType::Even(value) => {
                buf.extend(write_variable_integer(*value));
            }
        }
        buf
    }
}

#[cfg(test)]
mod tests {
    mod success {

        use crate::modules::moqt::messages::object::key_value_pair::{KeyValuePair, VariantType};

        #[test]
        fn packetize_and_depacketize_even_key() {
            let kv_pair = KeyValuePair {
                key: 0x3c, // PriorGroupIdGap (even)
                value: VariantType::Even(10),
            };

            let mut buf = kv_pair.packetize();
            let depacketized = KeyValuePair::depacketize(&mut buf).unwrap();

            assert_eq!(kv_pair.key, depacketized.key);
            match depacketized.value {
                VariantType::Even(v) => assert_eq!(v, 10),
                _ => panic!("Expected VariantType::Even"),
            }
            assert!(buf.is_empty());
        }

        #[test]
        fn packetize_and_depacketize_odd_key() {
            let kv_pair = KeyValuePair {
                key: 0x0b, // ImmutableExtensions (odd)
                value: VariantType::Odd(vec![0x01, 0x02, 0x03]),
            };

            let mut buf = kv_pair.packetize();
            let depacketized = KeyValuePair::depacketize(&mut buf).unwrap();

            assert_eq!(kv_pair.key, depacketized.key);
            match depacketized.value {
                VariantType::Odd(v) => assert_eq!(v, vec![0x01, 0x02, 0x03]),
                _ => panic!("Expected VariantType::Odd"),
            }
            assert!(buf.is_empty());
        }

        #[test]
        fn packetize_check_bytes_even_key() {
            let kv_pair = KeyValuePair {
                key: 0x3c,
                value: VariantType::Even(10),
            };

            let buf = kv_pair.packetize();

            // key (0x3c) + value (10)
            let expected_bytes = vec![0x3c, 0x0a];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }

        #[test]
        fn packetize_check_bytes_odd_key() {
            let kv_pair = KeyValuePair {
                key: 0x0b,
                value: VariantType::Odd(vec![0x01, 0x02, 0x03]),
            };

            let buf = kv_pair.packetize();

            // key (0x0b) + value length (3) + value ([0x01, 0x02, 0x03])
            let expected_bytes = vec![0x0b, 0x03, 0x01, 0x02, 0x03];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }
    }
}
