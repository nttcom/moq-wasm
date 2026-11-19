//! The encoding building blocks of draft-18: variable-length integers
//! (§1.4.1), Location (§1.4.2), Key-Value-Pairs (§1.4.3), Reason Phrase
//! (§1.4.4) and Track Namespace / Track Name (§2.4.1).

use bytes::{Buf, BufMut, Bytes};
use thiserror::Error;

use crate::modules::{
    extensions::name_reader::{self, NameError},
    moqt::{
        control_plane::{
            constants::TerminationErrorCode,
            control_messages::{
                key_value_pair::{KeyValuePair, VariantType},
                messages::parameters::{
                    location::Location,
                    track_properties::{ReceivedEncoding, TrackProperties, TrackProperty},
                },
            },
        },
        data_plane::object::{MAX_WIRE_DECLARED_LENGTH, declared_length},
    },
};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(crate) enum DecodeError {
    #[error("need more data to decode")]
    NeedMoreData,
    /// The session must close with `code`.
    #[error("{reason}")]
    Violation {
        code: TerminationErrorCode,
        reason: &'static str,
    },
    /// The stream ends: the length is over this endpoint's own resource limit
    /// (§13.5), not a violation of the draft, which sets none.
    #[error("declared length {0} exceeds {MAX_WIRE_DECLARED_LENGTH}")]
    LengthOverLimit(u64),
}

impl DecodeError {
    pub(crate) fn protocol_violation(reason: &'static str) -> Self {
        Self::Violation {
            code: TerminationErrorCode::ProtocolViolation,
            reason,
        }
    }
}

impl From<DecodeError> for std::io::Error {
    fn from(error: DecodeError) -> Self {
        std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
    }
}

pub(crate) fn take(buf: &mut impl Buf, len: usize) -> Result<Bytes, DecodeError> {
    if buf.remaining() < len {
        return Err(DecodeError::NeedMoreData);
    }
    Ok(buf.copy_to_bytes(len))
}

/// Reads a vi64 length a receiver holds in memory until all of it arrives.
pub(crate) fn get_declared_length(buf: &mut impl Buf) -> Result<usize, DecodeError> {
    let len = get_vi64(buf)?;
    declared_length(len).map_err(|_| DecodeError::LengthOverLimit(len))
}

/// Reads a vi64 length and that many bytes.
pub(crate) fn get_length_prefixed(buf: &mut impl Buf) -> Result<Bytes, DecodeError> {
    let len = get_declared_length(buf)?;
    take(buf, len)
}

pub(crate) fn get_u8(buf: &mut impl Buf) -> Result<u8, DecodeError> {
    buf.try_get_u8().map_err(|_| DecodeError::NeedMoreData)
}

/// Accepts any encoding length that can hold the value, not only the
/// shortest (§1.4.1).
pub(crate) fn get_vi64(buf: &mut impl Buf) -> Result<u64, DecodeError> {
    let first = *buf.chunk().first().ok_or(DecodeError::NeedMoreData)?;
    let len = first.leading_ones() as usize + 1;
    if buf.remaining() < len {
        return Err(DecodeError::NeedMoreData);
    }
    buf.advance(1);
    // The first byte holds no value bits from 8 bytes up.
    let mut value = u64::from(first & 0xFFu8.checked_shr(len as u32).unwrap_or(0));
    for _ in 1..len {
        value = (value << 8) | u64::from(buf.get_u8());
    }
    Ok(value)
}

/// Writes the shortest encoding.
pub(crate) fn put_vi64(buf: &mut impl BufMut, value: u64) {
    let len = (1..=8).find(|len| value < 1u64 << (7 * len)).unwrap_or(9);
    if len == 9 {
        buf.put_u8(0xFF);
        buf.put_u64(value);
        return;
    }
    let mut bytes = value.to_be_bytes();
    bytes[8 - len] |= !(0xFF >> (len - 1));
    buf.put_slice(&bytes[8 - len..]);
}

pub(crate) fn get_location(buf: &mut impl Buf) -> Result<Location, DecodeError> {
    Ok(Location {
        group_id: get_vi64(buf)?,
        object_id: get_vi64(buf)?,
    })
}

pub(crate) fn put_location(buf: &mut impl BufMut, location: &Location) {
    put_vi64(buf, location.group_id);
    put_vi64(buf, location.object_id);
}

/// §1.4.3: the longest value an odd type may carry.
const MAX_KEY_VALUE_LENGTH: u64 = u16::MAX as u64;

/// Reads pairs until `buf` is exhausted; the caller bounds it by the
/// enclosing length.
pub(crate) fn get_key_value_pairs(buf: &mut impl Buf) -> Result<Vec<KeyValuePair>, DecodeError> {
    let mut pairs = Vec::new();
    let mut key = 0u64;
    while buf.has_remaining() {
        key = next_key(key, get_vi64(buf)?)?;
        let value = get_value(buf, key)?;
        pairs.push(KeyValuePair { key, value });
    }
    Ok(pairs)
}

/// `get_key_value_pairs` for Track Properties, keeping the bytes each
/// arrived in so they can be sent again unchanged (§2.5).
pub(crate) fn get_track_properties(buf: &mut Bytes) -> Result<TrackProperties, DecodeError> {
    let mut properties = Vec::new();
    let mut key = 0u64;
    while buf.has_remaining() {
        let start = buf.clone();
        let delta_type = get_vi64(buf)?;
        let delta_type_bytes = start.slice(..start.len() - buf.len());
        key = next_key(key, delta_type)?;
        let start = buf.clone();
        let value = get_value(buf, key)?;
        properties.push(TrackProperty {
            pair: KeyValuePair { key, value },
            received: Some(ReceivedEncoding {
                delta_type,
                delta_type_bytes,
                value_bytes: start.slice(..start.len() - buf.len()),
            }),
        });
    }
    Ok(TrackProperties::received(properties))
}

fn next_key(previous: u64, delta_type: u64) -> Result<u64, DecodeError> {
    previous
        .checked_add(delta_type)
        .ok_or(DecodeError::protocol_violation(
            "key-value pair type overflows",
        ))
}

fn get_value(buf: &mut impl Buf, key: u64) -> Result<VariantType, DecodeError> {
    Ok(if key.is_multiple_of(2) {
        VariantType::Even(get_vi64(buf)?)
    } else {
        let len = get_vi64(buf)?;
        if len > MAX_KEY_VALUE_LENGTH {
            return Err(DecodeError::protocol_violation(
                "key-value pair value longer than 2^16-1 bytes",
            ));
        }
        VariantType::Odd(take(buf, len as usize)?)
    })
}

/// Fails when the keys are not in ascending order (types are delta-encoded)
/// or a value does not match its key's parity.
pub(crate) fn put_key_value_pairs(
    buf: &mut impl BufMut,
    pairs: &[KeyValuePair],
) -> anyhow::Result<()> {
    let mut prev = 0u64;
    for pair in pairs {
        put_vi64(buf, delta_type(prev, pair.key)?);
        put_value(buf, pair)?;
        prev = pair.key;
    }
    Ok(())
}

/// `put_key_value_pairs` for Track Properties: a received property goes out
/// in the bytes it arrived in, its Delta Type too while the type before it
/// is the same (§2.5).
pub(crate) fn put_track_properties(
    buf: &mut impl BufMut,
    properties: &TrackProperties,
) -> anyhow::Result<()> {
    let mut prev = 0u64;
    for property in properties.entries() {
        let delta = delta_type(prev, property.pair.key)?;
        match &property.received {
            Some(received) => {
                if received.delta_type == delta {
                    buf.put_slice(&received.delta_type_bytes);
                } else {
                    put_vi64(buf, delta);
                }
                buf.put_slice(&received.value_bytes);
            }
            None => {
                put_vi64(buf, delta);
                put_value(buf, &property.pair)?;
            }
        }
        prev = property.pair.key;
    }
    Ok(())
}

fn delta_type(previous: u64, key: u64) -> anyhow::Result<u64> {
    key.checked_sub(previous)
        .ok_or_else(|| anyhow::anyhow!("key-value pair types must ascend: {key}"))
}

fn put_value(buf: &mut impl BufMut, pair: &KeyValuePair) -> anyhow::Result<()> {
    match (&pair.value, pair.key % 2) {
        (VariantType::Even(value), 0) => put_vi64(buf, *value),
        (VariantType::Odd(value), 1) => {
            anyhow::ensure!(
                value.len() as u64 <= MAX_KEY_VALUE_LENGTH,
                "key-value pair value longer than 2^16-1 bytes"
            );
            put_vi64(buf, value.len() as u64);
            buf.put_slice(value);
        }
        _ => anyhow::bail!("key-value pair {} has a value of the wrong kind", pair.key),
    }
    Ok(())
}

impl From<NameError> for DecodeError {
    fn from(error: NameError) -> Self {
        match error {
            NameError::NeedMoreData => Self::NeedMoreData,
            NameError::ProtocolViolation(reason) => Self::protocol_violation(reason),
        }
    }
}

fn get_name_length(buf: &mut impl Buf) -> Option<u64> {
    get_vi64(buf).ok()
}

pub(crate) fn get_reason_phrase(buf: &mut impl Buf) -> Result<String, DecodeError> {
    Ok(name_reader::get_reason_phrase(buf, get_name_length)?)
}

/// Truncates at a character boundary to the 1024 bytes §1.4.4 allows.
pub(crate) fn put_reason_phrase(buf: &mut impl BufMut, reason: &str) {
    let reason = name_reader::truncate_reason_phrase(reason);
    put_vi64(buf, reason.len() as u64);
    buf.put_slice(reason.as_bytes());
}

pub(crate) fn get_track_namespace(buf: &mut impl Buf) -> Result<Vec<String>, DecodeError> {
    let namespace = name_reader::get_track_namespace(buf, get_name_length, 0..=32)?;
    if namespace.iter().any(String::is_empty) {
        return Err(DecodeError::protocol_violation(
            "empty track namespace field",
        ));
    }
    Ok(namespace)
}

/// Fails on an empty field, which §2.4.1 forbids; a namespace of no fields
/// is written.
pub(crate) fn put_track_namespace(
    buf: &mut impl BufMut,
    namespace: &[String],
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !namespace.iter().any(String::is_empty),
        "empty track namespace field"
    );
    put_vi64(buf, namespace.len() as u64);
    for field in namespace {
        put_vi64(buf, field.len() as u64);
        buf.put_slice(field.as_bytes());
    }
    Ok(())
}

pub(crate) fn get_full_track_name(
    buf: &mut impl Buf,
) -> Result<(Vec<String>, String), DecodeError> {
    let namespace = get_track_namespace(buf)?;
    let name = name_reader::get_track_name(buf, get_name_length, &namespace)?;
    Ok((namespace, name))
}

pub(crate) fn put_full_track_name(
    buf: &mut impl BufMut,
    namespace: &[String],
    name: &str,
) -> anyhow::Result<()> {
    put_track_namespace(buf, namespace)?;
    put_vi64(buf, name.len() as u64);
    buf.put_slice(name.as_bytes());
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use bytes::{Bytes, BytesMut};

    use super::{
        DecodeError, KeyValuePair, VariantType, get_full_track_name, get_key_value_pairs,
        get_reason_phrase, get_track_namespace, get_vi64, put_full_track_name, put_key_value_pairs,
        put_reason_phrase, put_track_namespace, put_vi64,
    };
    use crate::modules::moqt::control_plane::constants::TerminationErrorCode;

    pub(crate) fn violation(
        error: Result<impl std::fmt::Debug, DecodeError>,
    ) -> TerminationErrorCode {
        match error {
            Err(DecodeError::Violation { code, .. }) => code,
            other => panic!("expected a violation, got {other:?}"),
        }
    }

    #[test]
    fn the_example_integer_encodings_decode_to_their_values() {
        // Table 2 of §1.4.1.
        let examples: [(&[u8], u64); 8] = [
            (&[0x25], 37),
            (&[0x80, 0x25], 37),
            (&[0xbb, 0xbd], 15_293),
            (&[0xed, 0x7f, 0x3e, 0x7d], 226_442_877),
            (&[0xfa, 0xa1, 0xa0, 0xe4, 0x03, 0xd8], 2_893_212_287_960),
            (
                &[0xfc, 0x89, 0x98, 0xab, 0xc6, 0x6b, 0xc0],
                151_288_809_941_952,
            ),
            (
                &[0xfe, 0xfa, 0x31, 0x8f, 0xa8, 0xe3, 0xca, 0x11],
                70_423_237_261_249_041,
            ),
            (&[0xff; 9], u64::MAX),
        ];
        for (encoded, value) in examples {
            // Act
            let mut buf = encoded;
            let decoded = get_vi64(&mut buf);

            // Assert
            assert_eq!(decoded, Ok(value), "{encoded:02x?}");
            assert!(buf.is_empty(), "{encoded:02x?}");
        }
    }

    #[test]
    fn integers_encode_in_the_fewest_bytes_and_read_back() {
        let boundaries = [
            (0, 1),
            (127, 1),
            (128, 2),
            (16_383, 2),
            (16_384, 3),
            ((1 << 56) - 1, 8),
            (1 << 56, 9),
            (u64::MAX, 9),
        ];
        for (value, len) in boundaries {
            // Act
            let mut buf = BytesMut::new();
            put_vi64(&mut buf, value);
            let encoded_len = buf.len();
            let decoded = get_vi64(&mut buf.freeze());

            // Assert
            assert_eq!(encoded_len, len, "{value}");
            assert_eq!(decoded, Ok(value), "{value}");
        }
    }

    #[test]
    fn a_truncated_integer_needs_more_data() {
        // Act / Assert
        assert_eq!(
            get_vi64(&mut &[0xed, 0x7f, 0x3e][..]),
            Err(DecodeError::NeedMoreData)
        );
    }

    #[test]
    fn key_value_pairs_round_trip_with_delta_encoded_types() {
        // Arrange
        let pairs = vec![
            KeyValuePair {
                key: 2,
                value: VariantType::Even(300),
            },
            KeyValuePair {
                key: 7,
                value: VariantType::Odd(Bytes::from_static(b"value")),
            },
            KeyValuePair {
                key: 7,
                value: VariantType::Odd(Bytes::new()),
            },
        ];
        let mut buf = BytesMut::new();

        // Act
        put_key_value_pairs(&mut buf, &pairs).unwrap();
        let encoded = buf.clone().freeze();
        let decoded = get_key_value_pairs(&mut buf.freeze()).unwrap();

        // Assert: types 2, 7, 7 are sent as deltas 2, 5, 0
        assert_eq!(encoded[0], 2);
        assert_eq!(decoded, pairs);
    }

    #[test]
    fn key_value_pairs_out_of_order_are_not_encoded() {
        // Arrange
        let pairs = [
            KeyValuePair {
                key: 4,
                value: VariantType::Even(0),
            },
            KeyValuePair {
                key: 2,
                value: VariantType::Even(0),
            },
        ];

        // Act / Assert
        assert!(put_key_value_pairs(&mut BytesMut::new(), &pairs).is_err());
    }

    #[test]
    fn a_key_value_type_beyond_u64_is_a_protocol_violation() {
        // Arrange: type u64::MAX - 1 (even), then a delta of 2
        let mut buf = BytesMut::new();
        put_vi64(&mut buf, u64::MAX - 1);
        put_vi64(&mut buf, 0);
        put_vi64(&mut buf, 2);
        put_vi64(&mut buf, 0);

        // Act / Assert
        assert_eq!(
            violation(get_key_value_pairs(&mut buf.freeze())),
            TerminationErrorCode::ProtocolViolation
        );
    }

    #[test]
    fn a_key_value_of_u16_max_bytes_is_read() {
        // Arrange
        let mut buf = BytesMut::new();
        put_vi64(&mut buf, 1);
        put_vi64(&mut buf, u64::from(u16::MAX));
        buf.extend_from_slice(&[0; u16::MAX as usize]);

        // Act
        let pairs = get_key_value_pairs(&mut buf.freeze()).unwrap();

        // Assert
        assert!(matches!(
            &pairs[..],
            [KeyValuePair { key: 1, value: VariantType::Odd(value) }] if value.len() == usize::from(u16::MAX)
        ));
    }

    #[test]
    fn a_key_value_length_beyond_u16_is_a_protocol_violation() {
        // Arrange
        let mut buf = BytesMut::new();
        put_vi64(&mut buf, 1);
        put_vi64(&mut buf, 1 << 16);

        // Act / Assert
        assert_eq!(
            violation(get_key_value_pairs(&mut buf.freeze())),
            TerminationErrorCode::ProtocolViolation
        );
    }

    #[test]
    fn a_reason_phrase_longer_than_1024_bytes_is_cut_on_send_and_refused_on_receive() {
        // Arrange
        let long = "é".repeat(600);
        let mut sent = BytesMut::new();
        let mut received = BytesMut::new();
        put_vi64(&mut received, 1025);
        received.extend_from_slice(&[b'a'; 1025]);

        // Act
        put_reason_phrase(&mut sent, &long);
        let read_back = get_reason_phrase(&mut sent.freeze()).unwrap();

        // Assert
        assert_eq!(read_back.len(), 1024);
        assert_eq!(
            violation(get_reason_phrase(&mut received.freeze())),
            TerminationErrorCode::ProtocolViolation
        );
    }

    #[test]
    fn a_reason_phrase_is_cut_before_a_character_the_1024_byte_limit_splits() {
        // Arrange: the 1024-byte limit ends inside the 512th "é"
        let long = format!("a{}", "é".repeat(600));
        let mut sent = BytesMut::new();

        // Act
        put_reason_phrase(&mut sent, &long);
        let read_back = get_reason_phrase(&mut sent.freeze()).unwrap();

        // Assert
        assert_eq!(read_back, long[..1023]);
    }

    #[test]
    fn a_non_utf8_reason_phrase_reads_with_replacement_characters() {
        // Act / Assert
        assert_eq!(
            get_reason_phrase(&mut &[2, b'a', 0xff][..]),
            Ok("a\u{FFFD}".to_string())
        );
    }

    #[test]
    fn a_full_track_name_round_trips() {
        // Arrange
        let namespace = vec!["live".to_string(), "room".to_string()];
        let mut buf = BytesMut::new();

        // Act
        put_full_track_name(&mut buf, &namespace, "video").unwrap();
        let decoded = get_full_track_name(&mut buf.freeze());

        // Assert
        assert_eq!(decoded, Ok((namespace, "video".to_string())));
    }

    #[test]
    fn a_namespace_or_full_track_name_with_an_empty_field_is_not_encoded() {
        // Arrange
        let namespace = vec!["live".to_string(), String::new()];
        let mut buf = BytesMut::new();

        // Act
        let namespace_result = put_track_namespace(&mut buf, &namespace);
        let full_name_result = put_full_track_name(&mut buf, &namespace, "video");

        // Assert
        assert!(namespace_result.is_err());
        assert!(full_name_result.is_err());
        assert!(buf.is_empty());
    }

    #[test]
    fn a_namespace_of_no_fields_round_trips() {
        // Arrange
        let mut buf = BytesMut::new();

        // Act
        put_track_namespace(&mut buf, &[]).unwrap();
        let decoded = get_track_namespace(&mut buf.freeze());

        // Assert
        assert_eq!(decoded, Ok(Vec::new()));
    }

    #[test]
    fn an_empty_namespace_field_and_more_than_32_fields_are_protocol_violations() {
        // Arrange
        let mut empty_field = BytesMut::new();
        put_vi64(&mut empty_field, 1);
        put_vi64(&mut empty_field, 0);
        let mut too_many = BytesMut::new();
        put_vi64(&mut too_many, 33);

        // Act / Assert
        assert_eq!(
            violation(get_track_namespace(&mut empty_field.freeze())),
            TerminationErrorCode::ProtocolViolation
        );
        assert_eq!(
            violation(get_track_namespace(&mut too_many.freeze())),
            TerminationErrorCode::ProtocolViolation
        );
    }

    #[test]
    fn a_full_track_name_beyond_4096_bytes_is_a_protocol_violation() {
        // Arrange
        let namespace = vec!["n".repeat(4000)];
        let mut buf = BytesMut::new();
        put_full_track_name(&mut buf, &namespace, &"t".repeat(97)).unwrap();

        // Act / Assert
        assert_eq!(
            violation(get_full_track_name(&mut buf.freeze())),
            TerminationErrorCode::ProtocolViolation
        );
    }

    mod properties {
        use bytes::{Buf, BufMut, Bytes, BytesMut};
        use proptest::{
            collection::vec,
            option,
            prelude::*,
            sample::{Index, select},
        };

        use super::super::{get_track_properties, get_vi64, put_track_properties, put_vi64};
        use crate::modules::moqt::control_plane::control_messages::{
            key_value_pair::{KeyValuePair, VariantType},
            messages::parameters::{
                group_order::GroupOrder,
                track_properties::{
                    DEFAULT_PUBLISHER_GROUP_ORDER, DEFAULT_PUBLISHER_PRIORITY, DYNAMIC_GROUPS,
                    IMMUTABLE_PROPERTIES, MAX_CACHE_DURATION, OBJECT_DELIVERY_TIMEOUT,
                    TrackProperties,
                },
            },
        };
        use crate::modules::test_support::object_strategies::vi64;

        /// Writes `value` in `len` bytes, which must hold it (§1.4.1).
        fn put_vi64_in(buf: &mut BytesMut, value: u64, len: usize) {
            if len == 9 {
                buf.put_u8(0xFF);
                buf.put_u64(value);
                return;
            }
            let mut bytes = value.to_be_bytes();
            bytes[8 - len] |= !(0xFF >> (len - 1));
            buf.put_slice(&bytes[8 - len..]);
        }

        fn shortest_len(value: u64) -> usize {
            (1..=8).find(|len| value < 1u64 << (7 * len)).unwrap_or(9)
        }

        fn shortest(value: u64) -> Bytes {
            let mut buf = BytesMut::new();
            put_vi64(&mut buf, value);
            buf.freeze()
        }

        /// Writes `value` in its shortest encoding plus `padding` bytes, up
        /// to the 9-byte form.
        fn put_vi64_padded(buf: &mut BytesMut, value: u64, padding: usize) {
            put_vi64_in(buf, value, (shortest_len(value) + padding).min(9));
        }

        /// How many bytes an integer takes beyond its shortest encoding:
        /// mostly none, otherwise any up to the 9-byte form.
        fn padding() -> impl Strategy<Value = usize> {
            prop_oneof![3 => Just(0), 1 => 1..=8usize]
        }

        /// Mostly the types the accessors read, so a list often repeats one;
        /// otherwise small types or any type.
        fn property_type() -> impl Strategy<Value = u64> {
            prop_oneof![
                3 => select(vec![
                    OBJECT_DELIVERY_TIMEOUT,
                    MAX_CACHE_DURATION,
                    IMMUTABLE_PROPERTIES,
                    DEFAULT_PUBLISHER_PRIORITY,
                    DEFAULT_PUBLISHER_GROUP_ORDER,
                    DYNAMIC_GROUPS,
                ]),
                1 => 0..0x40u64,
                1 => vi64(),
            ]
        }

        /// Mostly small values, which the setters often set again; otherwise any.
        fn property_value() -> impl Strategy<Value = u64> {
            prop_oneof![3 => 0..2u64, 1 => vi64()]
        }

        /// A received property and the bytes its Delta Type and value
        /// arrived in.
        #[derive(Debug, Clone)]
        struct ReceivedProperty {
            pair: KeyValuePair,
            delta_type_bytes: Bytes,
            value_bytes: Bytes,
        }

        /// Track Properties in ascending type order, each integer in any
        /// encoding that holds it, and the properties they carry.
        fn received_properties() -> impl Strategy<Value = (Bytes, Vec<ReceivedProperty>)> {
            let property = (
                property_type(),
                property_value(),
                vec(any::<u8>(), 0..6),
                padding(),
                padding(),
            );
            vec(property, 0..12).prop_map(|mut properties| {
                properties.sort_by_key(|(key, ..)| *key);
                let mut bytes = BytesMut::new();
                let mut received = Vec::new();
                let mut previous = 0;
                for (key, integer, contents, delta_padding, value_padding) in properties {
                    let mut delta_type_bytes = BytesMut::new();
                    put_vi64_padded(&mut delta_type_bytes, key - previous, delta_padding);
                    let mut value_bytes = BytesMut::new();
                    let value = if key % 2 == 0 {
                        put_vi64_padded(&mut value_bytes, integer, value_padding);
                        VariantType::Even(integer)
                    } else {
                        put_vi64_padded(&mut value_bytes, contents.len() as u64, value_padding);
                        value_bytes.put_slice(&contents);
                        VariantType::Odd(Bytes::from(contents))
                    };
                    bytes.put_slice(&delta_type_bytes);
                    bytes.put_slice(&value_bytes);
                    received.push(ReceivedProperty {
                        pair: KeyValuePair { key, value },
                        delta_type_bytes: delta_type_bytes.freeze(),
                        value_bytes: value_bytes.freeze(),
                    });
                    previous = key;
                }
                (bytes.freeze(), received)
            })
        }

        #[derive(Debug, Clone)]
        enum Setter {
            DeliveryTimeout(Option<u64>),
            MaxCacheDuration(Option<u64>),
            GroupOrder(Option<GroupOrder>),
            DynamicGroups(bool),
        }

        fn setter() -> impl Strategy<Value = Setter> {
            let group_order = option::of(select(vec![
                GroupOrder::Publisher,
                GroupOrder::Ascending,
                GroupOrder::Descending,
            ]));
            prop_oneof![
                option::of(property_value()).prop_map(Setter::DeliveryTimeout),
                option::of(property_value()).prop_map(Setter::MaxCacheDuration),
                group_order.prop_map(Setter::GroupOrder),
                any::<bool>().prop_map(Setter::DynamicGroups),
            ]
        }

        impl Setter {
            fn apply(&self, properties: &mut TrackProperties) {
                match *self {
                    Self::DeliveryTimeout(value) => properties.set_delivery_timeout(value),
                    Self::MaxCacheDuration(value) => properties.set_max_cache_duration(value),
                    Self::GroupOrder(value) => properties.set_group_order(value),
                    Self::DynamicGroups(value) => properties.set_dynamic_groups(value),
                }
            }

            /// The type it sets and the one value of that type it leaves,
            /// `None` leaving none.
            fn sets(&self) -> (u64, Option<u64>) {
                match *self {
                    Self::DeliveryTimeout(value) => (OBJECT_DELIVERY_TIMEOUT, value),
                    Self::MaxCacheDuration(value) => (MAX_CACHE_DURATION, value),
                    Self::GroupOrder(value) => (
                        DEFAULT_PUBLISHER_GROUP_ORDER,
                        match value {
                            Some(GroupOrder::Ascending) => Some(1),
                            Some(GroupOrder::Descending) => Some(2),
                            Some(GroupOrder::Publisher) | None => None,
                        },
                    ),
                    Self::DynamicGroups(value) => (DYNAMIC_GROUPS, value.then_some(1)),
                }
            }
        }

        /// A property expected to be sent: the index of the received
        /// property it is, or `None` for one a setter added.
        struct Expected {
            pair: KeyValuePair,
            received: Option<usize>,
        }

        /// A setter leaves the properties of its type alone when they are
        /// already just its value, and otherwise replaces them with its value
        /// after the smaller types.
        fn expect_set(expected: &mut Vec<Expected>, (key, value): (u64, Option<u64>)) {
            let wanted = value.map(|value| KeyValuePair {
                key,
                value: VariantType::Even(value),
            });
            let held: Vec<&KeyValuePair> = expected
                .iter()
                .map(|property| &property.pair)
                .filter(|pair| pair.key == key)
                .collect();
            if held == Vec::from_iter(wanted.as_ref()) {
                return;
            }
            expected.retain(|property| property.pair.key != key);
            if let Some(pair) = wanted {
                let at = expected.partition_point(|property| property.pair.key < key);
                expected.insert(
                    at,
                    Expected {
                        pair,
                        received: None,
                    },
                );
            }
        }

        proptest! {
            #[test]
            fn an_integer_reads_back_however_its_bytes_are_split(value in vi64(), split in any::<Index>()) {
                // Arrange
                let mut bytes = BytesMut::new();
                put_vi64(&mut bytes, value);
                let (head, tail) = bytes.split_at(split.index(bytes.len() + 1));
                let mut chunks = head.chain(tail);

                // Act
                let decoded = get_vi64(&mut chunks);

                // Assert
                prop_assert_eq!(decoded.ok(), Some(value));
                prop_assert_eq!(chunks.remaining(), 0);
            }

            #[test]
            fn set_track_properties_go_out_afresh_and_the_others_in_their_received_bytes(
                (bytes, received) in received_properties(),
                setters in vec(setter(), 0..4),
            ) {
                // Arrange
                let mut properties = get_track_properties(&mut bytes.clone()).unwrap();
                prop_assert!(properties.iter().eq(received.iter().map(|property| &property.pair)));
                let mut expected: Vec<Expected> = received
                    .iter()
                    .enumerate()
                    .map(|(index, property)| Expected {
                        pair: property.pair.clone(),
                        received: Some(index),
                    })
                    .collect();

                // Act
                for setter in &setters {
                    setter.apply(&mut properties);
                    expect_set(&mut expected, setter.sets());
                }
                let mut sent = BytesMut::new();
                put_track_properties(&mut sent, &properties).unwrap();

                // Assert: §2.5, a property no setter changed goes out in the
                // bytes it arrived in, its Delta Type too while the type
                // before it is the same; a set one in the shortest encoding
                if setters.is_empty() {
                    prop_assert_eq!(&sent[..], &bytes[..]);
                }
                let sent = get_track_properties(&mut sent.freeze()).unwrap();
                prop_assert!(sent.iter().eq(expected.iter().map(|property| &property.pair)));
                let mut previous_key = 0;
                for (entry, property) in sent.entries().iter().zip(&expected) {
                    let encoding = entry.received.as_ref().unwrap();
                    let delta = property.pair.key - previous_key;
                    match property.received {
                        Some(index) => {
                            let received_previous_key =
                                index.checked_sub(1).map_or(0, |previous| received[previous].pair.key);
                            let delta_type_bytes = if received_previous_key == previous_key {
                                received[index].delta_type_bytes.clone()
                            } else {
                                shortest(delta)
                            };
                            prop_assert_eq!(&encoding.delta_type_bytes, &delta_type_bytes);
                            prop_assert_eq!(&encoding.value_bytes, &received[index].value_bytes);
                        }
                        None => {
                            let VariantType::Even(value) = property.pair.value else {
                                unreachable!("the setters set integer properties only");
                            };
                            prop_assert_eq!(&encoding.delta_type_bytes, &shortest(delta));
                            prop_assert_eq!(&encoding.value_bytes, &shortest(value));
                        }
                    }
                    previous_key = property.pair.key;
                }
            }
        }
    }
}
