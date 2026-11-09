use bytes::{Buf, Bytes};

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt},
    moqt::{
        control_plane::control_messages::key_value_pair::{KeyValuePair, VariantType},
        data_plane::object::{declared_length, decode_error::DecodeError},
    },
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

    /// Fails with `NeedMoreData` until the length and the whole block have
    /// arrived, and with `Fatal` when the length exceeds 16 MiB
    /// (`MAX_WIRE_DECLARED_LENGTH`) or a Key-Value-Pair ends after the block.
    pub fn decode(cursor: &mut impl Buf) -> Result<Self, DecodeError> {
        let byte_length = declared_length(cursor.try_get_varint()?)?;
        if cursor.remaining() < byte_length {
            return Err(DecodeError::NeedMoreData);
        }
        let mut block = cursor.copy_to_bytes(byte_length);
        let mut key_value_pairs = Vec::new();
        while block.has_remaining() {
            let kv_pair = KeyValuePair::decode(&mut block)
                .map_err(|error| DecodeError::Fatal(format!("extension header: {error:#}")))?;
            key_value_pairs.push(kv_pair);
        }
        Ok(Self { key_value_pairs })
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
    use crate::modules::moqt::data_plane::object::MAX_WIRE_DECLARED_LENGTH;

    #[test]
    fn round_trip_preserves_unsupported_headers_in_order() {
        // Keys 0x10 and 0x0D are LOC properties the transport does not
        // interpret; draft-14 §10.2.1.2 requires them to be forwarded unmodified.
        let headers = ExtensionHeaders::new(vec![
            KeyValuePair {
                key: 0x10,
                value: VariantType::Even(123),
            },
            KeyValuePair {
                key: ExtensionHeaderType::PriorGroupIdGap as u64,
                value: VariantType::Even(7),
            },
            KeyValuePair {
                key: 0x0D,
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

    #[test]
    fn decode_truncated_block_needs_more_data() {
        // Arrange: length claims 4 bytes but only 2 follow
        let bytes = [0x04_u8, 0x3c, 0x05];
        let mut cursor = std::io::Cursor::new(&bytes[..]);

        // Act
        let result = ExtensionHeaders::decode(&mut cursor);

        // Assert
        assert_eq!(result, Err(DecodeError::NeedMoreData));
    }

    #[test]
    fn decode_kv_pair_overrunning_complete_block_is_fatal() {
        // Arrange: a 2-byte block (odd key 0x0b, value length 5) whose 5 value bytes follow outside it
        let bytes = [0x02_u8, 0x0b, 0x05, 1, 2, 3, 4, 5];
        let mut cursor = std::io::Cursor::new(&bytes[..]);

        // Act
        let result = ExtensionHeaders::decode(&mut cursor);

        // Assert
        assert!(matches!(result, Err(DecodeError::Fatal(_))));
    }

    #[test]
    fn decode_empty_input_needs_more_data() {
        // Arrange
        let bytes: [u8; 0] = [];
        let mut cursor = std::io::Cursor::new(&bytes[..]);

        // Act
        let result = ExtensionHeaders::decode(&mut cursor);

        // Assert
        assert_eq!(result, Err(DecodeError::NeedMoreData));
    }

    #[test]
    fn decode_length_over_limit_is_fatal() {
        // Arrange: no block bytes follow, so without the limit this is NeedMoreData
        let mut buf = bytes::BytesMut::new();
        buf.put_varint(MAX_WIRE_DECLARED_LENGTH + 1);

        // Act
        let result = ExtensionHeaders::decode(&mut buf);

        // Assert
        assert!(matches!(result, Err(DecodeError::Fatal(_))));
    }
}
