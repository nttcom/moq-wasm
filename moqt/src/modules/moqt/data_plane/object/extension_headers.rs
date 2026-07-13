use bytes::{Buf, Bytes};

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::key_value_pair::{KeyValuePair, VariantType},
};

pub enum ExtensionHeaderType {
    PriorGroupIdGap = 0x3c,
    PriorObjectIdGap = 0x3e,
    ImmutableExtensions = 0xb,
}

// Every Key-Value-Pair is preserved in receive order and re-encoded unchanged:
// draft-ietf-moq-transport-14 §10.2.1.2 requires unsupported extension headers
// to be forwarded and cached without modification. Higher layers (e.g. LOC)
// assign meaning to specific keys; the transport only exposes accessors for the
// header types it uses itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExtensionHeaders {
    pub key_value_pairs: Vec<KeyValuePair>,
}

impl ExtensionHeaders {
    pub fn new(key_value_pairs: Vec<KeyValuePair>) -> Self {
        Self { key_value_pairs }
    }

    pub fn from_immutable_extensions(values: Vec<Bytes>) -> Self {
        let mut headers = Self::default();
        for value in values {
            headers.push_immutable_extension(value);
        }
        headers
    }

    pub fn decode(cursor: &mut impl Buf) -> Option<Self> {
        let byte_length = cursor
            .try_get_varint()
            .log_context("extension headers length")
            .ok()? as usize;
        if cursor.remaining() < byte_length {
            return None;
        }
        let end_remaining = cursor.remaining() - byte_length;
        let mut key_value_pairs = Vec::new();
        while cursor.remaining() > end_remaining {
            key_value_pairs.push(KeyValuePair::decode(cursor)?);
        }
        Some(Self { key_value_pairs })
    }

    pub fn encode(&self) -> bytes::BytesMut {
        let mut kv_buf = bytes::BytesMut::new();
        for kv_pair in &self.key_value_pairs {
            kv_buf.unsplit(kv_pair.encode());
        }
        let mut buf = bytes::BytesMut::new();
        buf.put_varint(kv_buf.len() as u64);
        buf.unsplit(kv_buf);
        buf
    }

    pub fn prior_group_id_gap(&self) -> Vec<u64> {
        self.even_values(ExtensionHeaderType::PriorGroupIdGap as u64)
    }

    pub fn prior_object_id_gap(&self) -> Vec<u64> {
        self.even_values(ExtensionHeaderType::PriorObjectIdGap as u64)
    }

    pub fn immutable_extensions(&self) -> Vec<Bytes> {
        self.key_value_pairs
            .iter()
            .filter(|kv_pair| kv_pair.key == ExtensionHeaderType::ImmutableExtensions as u64)
            .filter_map(|kv_pair| match &kv_pair.value {
                VariantType::Odd(value) => Some(value.clone()),
                VariantType::Even(_) => None,
            })
            .collect()
    }

    pub fn push_prior_group_id_gap(&mut self, gap: u64) {
        self.push_even(ExtensionHeaderType::PriorGroupIdGap as u64, gap);
    }

    pub fn push_prior_object_id_gap(&mut self, gap: u64) {
        self.push_even(ExtensionHeaderType::PriorObjectIdGap as u64, gap);
    }

    pub fn push_immutable_extension(&mut self, value: Bytes) {
        self.key_value_pairs.push(KeyValuePair {
            key: ExtensionHeaderType::ImmutableExtensions as u64,
            value: VariantType::Odd(value),
        });
    }

    fn even_values(&self, key: u64) -> Vec<u64> {
        self.key_value_pairs
            .iter()
            .filter(|kv_pair| kv_pair.key == key)
            .filter_map(|kv_pair| match &kv_pair.value {
                VariantType::Even(value) => Some(*value),
                VariantType::Odd(_) => None,
            })
            .collect()
    }

    fn push_even(&mut self, key: u64, value: u64) {
        self.key_value_pairs.push(KeyValuePair {
            key,
            value: VariantType::Even(value),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_unsupported_headers_in_order() {
        // Keys 2 and 13 are LOC extension header types the transport does not
        // interpret; draft-14 §10.2.1.2 requires them to be forwarded unmodified.
        let headers = ExtensionHeaders::new(vec![
            KeyValuePair {
                key: 2,
                value: VariantType::Even(123),
            },
            KeyValuePair {
                key: ExtensionHeaderType::PriorGroupIdGap as u64,
                value: VariantType::Even(7),
            },
            KeyValuePair {
                key: 13,
                value: VariantType::Odd(Bytes::from(vec![0xAA, 0xBB])),
            },
        ]);

        let mut encoded = headers.encode();
        let decoded = ExtensionHeaders::decode(&mut encoded).unwrap();

        assert_eq!(decoded, headers);
        assert_eq!(decoded.prior_group_id_gap(), vec![7]);
    }

    #[test]
    fn accessors_filter_by_header_type() {
        let mut headers = ExtensionHeaders::default();
        headers.push_prior_group_id_gap(1);
        headers.push_prior_object_id_gap(2);
        headers.push_immutable_extension(Bytes::from(vec![9]));

        assert_eq!(headers.prior_group_id_gap(), vec![1]);
        assert_eq!(headers.prior_object_id_gap(), vec![2]);
        assert_eq!(headers.immutable_extensions(), vec![Bytes::from(vec![9])]);
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::data_plane::object::extension_headers::ExtensionHeaders;
        use bytes::Bytes;

        #[test]
        fn packetize_and_depacketize_all_header_types() {
            let headers = ExtensionHeaders {
                prior_group_id_gap: vec![1, 2],
                prior_object_id_gap: vec![3],
                immutable_extensions: vec![Bytes::from(vec![0xaa, 0xbb])],
            };

            let buf = headers.encode();
            let mut cursor = std::io::Cursor::new(&buf[..]);
            let depacketized = ExtensionHeaders::decode(&mut cursor).unwrap();

            assert_eq!(depacketized, headers);
            assert_eq!(cursor.position() as usize, buf.len());
        }

        #[test]
        fn packetize_and_depacketize_empty() {
            let headers = ExtensionHeaders {
                prior_group_id_gap: vec![],
                prior_object_id_gap: vec![],
                immutable_extensions: vec![],
            };

            let buf = headers.encode();
            let mut cursor = std::io::Cursor::new(&buf[..]);
            let depacketized = ExtensionHeaders::decode(&mut cursor).unwrap();

            assert_eq!(depacketized, headers);
        }

        #[test]
        fn depacketize_ignores_unknown_even_key() {
            // length (4) + unknown even key (0x10) + value (7)
            // + PriorGroupIdGap key (0x3c) + value (5)
            let bytes = [0x04_u8, 0x10, 0x07, 0x3c, 0x05];
            let mut cursor = std::io::Cursor::new(&bytes[..]);
            let depacketized = ExtensionHeaders::decode(&mut cursor).unwrap();

            assert_eq!(depacketized.prior_group_id_gap, vec![5]);
            assert!(depacketized.prior_object_id_gap.is_empty());
            assert!(depacketized.immutable_extensions.is_empty());
        }
    }

    mod failure {
        use crate::modules::moqt::data_plane::object::extension_headers::ExtensionHeaders;

        #[test]
        fn depacketize_truncated_block() {
            // length claims 4 bytes but only 2 follow
            let bytes = [0x04_u8, 0x3c, 0x05];
            let mut cursor = std::io::Cursor::new(&bytes[..]);

            assert!(ExtensionHeaders::decode(&mut cursor).is_none());
        }

        #[test]
        fn depacketize_truncated_inner_kv_pair() {
            // length (2) + odd key (0x0b) + value length (5) with no value bytes
            let bytes = [0x02_u8, 0x0b, 0x05];
            let mut cursor = std::io::Cursor::new(&bytes[..]);

            assert!(ExtensionHeaders::decode(&mut cursor).is_none());
        }

        #[test]
        fn depacketize_empty_input() {
            let bytes: [u8; 0] = [];
            let mut cursor = std::io::Cursor::new(&bytes[..]);

            assert!(ExtensionHeaders::decode(&mut cursor).is_none());
        }
    }
}
