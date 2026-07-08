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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionHeaders {
    pub prior_group_id_gap: Vec<u64>,
    pub prior_object_id_gap: Vec<u64>,
    pub immutable_extensions: Vec<Bytes>,
}

impl ExtensionHeaders {
    pub fn decode(cursor: &mut impl Buf) -> Option<Self> {
        let mut kv_pairs = Vec::new();
        let byte_length = cursor
            .try_get_varint()
            .log_context("extension headers length")
            .ok()? as usize;
        if cursor.remaining() < byte_length {
            return None;
        }
        let end_remaining = cursor.remaining() - byte_length;
        while cursor.remaining() > end_remaining {
            let key_value_pair = KeyValuePair::decode(cursor)?;
            kv_pairs.push(key_value_pair);
        }
        let prior_group_id_gap = kv_pairs
            .iter()
            .filter(|item| item.key == ExtensionHeaderType::PriorGroupIdGap as u64)
            .map(|kv_pair| match &kv_pair.value {
                VariantType::Odd(_) => unreachable!(),
                VariantType::Even(v) => *v,
            })
            .collect();
        let prior_object_id_gap = kv_pairs
            .iter()
            .filter(|item| item.key == ExtensionHeaderType::PriorObjectIdGap as u64)
            .map(|kv_pair| match &kv_pair.value {
                VariantType::Odd(_) => unreachable!(),
                VariantType::Even(v) => *v,
            })
            .collect();
        let immutable_extensions = kv_pairs
            .iter()
            .filter(|kv_pair| kv_pair.key == ExtensionHeaderType::ImmutableExtensions as u64)
            .map(|kv_pair| match &kv_pair.value {
                VariantType::Odd(value) => value.clone(),
                VariantType::Even(_) => unreachable!(),
            })
            .collect();

        Some(Self {
            prior_group_id_gap,
            prior_object_id_gap,
            immutable_extensions,
        })
    }

    pub fn encode(&self) -> bytes::BytesMut {
        let mut kv_buf = bytes::BytesMut::new();
        for gap in &self.prior_group_id_gap {
            kv_buf.put_varint(ExtensionHeaderType::PriorGroupIdGap as u64);
            kv_buf.put_varint(*gap);
        }
        for gap in &self.prior_object_id_gap {
            kv_buf.put_varint(ExtensionHeaderType::PriorObjectIdGap as u64);
            kv_buf.put_varint(*gap);
        }
        for ext in &self.immutable_extensions {
            kv_buf.put_varint(ExtensionHeaderType::ImmutableExtensions as u64);
            kv_buf.put_varint(ext.len() as u64);
            kv_buf.extend_from_slice(ext);
        }
        let mut buf = bytes::BytesMut::new();
        buf.put_varint(kv_buf.len() as u64);
        buf.unsplit(kv_buf);
        buf
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
