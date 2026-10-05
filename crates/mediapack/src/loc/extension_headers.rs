use moqt::{ExtensionHeaders, KeyValuePair, VariantType};

use crate::loc::object::{LocExtension, LocValue};

/// draft-ietf-moq-loc-01 §2.3: each LOC header extension is its own MoQT
/// extension header keyed by the LOC id; even ids carry varints, odd ids bytes.
pub fn to_extension_headers(extensions: &[LocExtension]) -> ExtensionHeaders {
    ExtensionHeaders::new(
        extensions
            .iter()
            .map(|extension| KeyValuePair {
                key: extension.id,
                value: match &extension.value {
                    LocValue::Varint(value) => VariantType::Even(*value),
                    LocValue::Bytes(bytes) => VariantType::Odd(bytes.clone()),
                },
            })
            .collect(),
    )
}

pub fn from_extension_headers(headers: &ExtensionHeaders) -> Vec<LocExtension> {
    headers
        .key_value_pairs
        .iter()
        .map(|kv_pair| match &kv_pair.value {
            VariantType::Even(value) => LocExtension::varint(kv_pair.key, *value),
            VariantType::Odd(bytes) => LocExtension::bytes(kv_pair.key, bytes.clone()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;
    use crate::loc::object::{CAPTURE_TIMESTAMP_ID, VIDEO_CONFIG_ID};

    fn extensions() -> Vec<LocExtension> {
        vec![
            LocExtension::varint(CAPTURE_TIMESTAMP_ID, 7),
            LocExtension::bytes(VIDEO_CONFIG_ID, Bytes::from_static(&[1, 2])),
            LocExtension::bytes(21, Bytes::from_static(&[0xFF])),
        ]
    }

    #[test]
    fn maps_each_extension_to_a_key_value_pair_by_parity() {
        // Act
        let headers = to_extension_headers(&extensions());

        // Assert
        assert_eq!(
            headers.key_value_pairs,
            vec![
                KeyValuePair {
                    key: CAPTURE_TIMESTAMP_ID,
                    value: VariantType::Even(7),
                },
                KeyValuePair {
                    key: VIDEO_CONFIG_ID,
                    value: VariantType::Odd(Bytes::from_static(&[1, 2])),
                },
                KeyValuePair {
                    key: 21,
                    value: VariantType::Odd(Bytes::from_static(&[0xFF])),
                },
            ]
        );
    }

    #[test]
    fn round_trips_through_extension_headers_preserving_order_and_unknown_ids() {
        // Act
        let decoded = from_extension_headers(&to_extension_headers(&extensions()));

        // Assert
        assert_eq!(decoded, extensions());
    }
}
