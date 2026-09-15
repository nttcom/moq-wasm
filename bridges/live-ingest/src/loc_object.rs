use mediapack::loc::{LocExtension, LocObject, LocValue};
use moqt::{ExtensionHeaders, KeyValuePair, VariantType};

/// draft-ietf-moq-loc-01 §2.3: each LOC header extension is its own MoQT
/// extension header keyed by the LOC id; even ids carry varints, odd ids bytes.
pub(crate) fn extension_headers(object: &LocObject) -> ExtensionHeaders {
    ExtensionHeaders::new(object.extensions.iter().map(key_value_pair).collect())
}

fn key_value_pair(extension: &LocExtension) -> KeyValuePair {
    KeyValuePair {
        key: extension.id,
        value: match &extension.value {
            LocValue::Varint(value) => VariantType::Even(*value),
            LocValue::Bytes(bytes) => VariantType::Odd(bytes.clone()),
        },
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;

    #[test]
    fn maps_each_extension_to_a_key_value_pair_by_parity() {
        // Arrange
        let object = LocObject {
            extensions: vec![
                LocExtension::varint(2, 7),
                LocExtension::bytes(13, Bytes::from_static(&[1, 2])),
            ],
            payload: Bytes::new(),
        };

        // Act
        let headers = extension_headers(&object);

        // Assert
        assert_eq!(
            headers.key_value_pairs,
            vec![
                KeyValuePair {
                    key: 2,
                    value: VariantType::Even(7),
                },
                KeyValuePair {
                    key: 13,
                    value: VariantType::Odd(Bytes::from_static(&[1, 2])),
                },
            ]
        );
    }
}
