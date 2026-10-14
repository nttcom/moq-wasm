//! The encoding building blocks of draft-18: variable-length integers
//! (§1.4.1), Location (§1.4.2), Key-Value-Pairs (§1.4.3), Reason Phrase
//! (§1.4.4) and Track Namespace / Track Name (§2.4.1).

use bytes::{Buf, BufMut, Bytes};
use thiserror::Error;

use crate::modules::moqt::control_plane::{
    constants::TerminationErrorCode, control_messages::messages::parameters::location::Location,
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
}

impl DecodeError {
    fn protocol_violation(reason: &'static str) -> Self {
        Self::Violation {
            code: TerminationErrorCode::ProtocolViolation,
            reason,
        }
    }
}

fn take(buf: &mut impl Buf, len: usize) -> Result<Bytes, DecodeError> {
    if buf.remaining() < len {
        return Err(DecodeError::NeedMoreData);
    }
    Ok(buf.copy_to_bytes(len))
}

/// Accepts any encoding length that can hold the value, not only the
/// shortest (§1.4.1).
pub(crate) fn get_vi64(buf: &mut impl Buf) -> Result<u64, DecodeError> {
    let first = *buf.chunk().first().ok_or(DecodeError::NeedMoreData)?;
    let len = (first.leading_ones() as usize + 1).min(9);
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
    let bytes = value.to_be_bytes();
    let mut encoded = bytes[8 - len..].to_vec();
    encoded[0] |= !(0xFF >> (len - 1));
    buf.put_slice(&encoded);
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyValue {
    /// An even type.
    Varint(u64),
    /// An odd type.
    Bytes(Bytes),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyValuePair {
    pub(crate) key: u64,
    pub(crate) value: KeyValue,
}

/// Reads pairs until `buf` is exhausted; the caller bounds it by the
/// enclosing length.
pub(crate) fn get_key_value_pairs(buf: &mut impl Buf) -> Result<Vec<KeyValuePair>, DecodeError> {
    let mut pairs = Vec::new();
    let mut key = 0u64;
    while buf.has_remaining() {
        key = key
            .checked_add(get_vi64(buf)?)
            .ok_or(DecodeError::protocol_violation(
                "key-value pair type overflows",
            ))?;
        let value = if key.is_multiple_of(2) {
            KeyValue::Varint(get_vi64(buf)?)
        } else {
            let len = get_vi64(buf)?;
            if len > MAX_KEY_VALUE_LENGTH {
                return Err(DecodeError::protocol_violation(
                    "key-value pair value longer than 2^16-1 bytes",
                ));
            }
            KeyValue::Bytes(take(buf, len as usize)?)
        };
        pairs.push(KeyValuePair { key, value });
    }
    Ok(pairs)
}

/// Fails when the keys are not in ascending order (types are delta-encoded)
/// or a value does not match its key's parity.
pub(crate) fn put_key_value_pairs(
    buf: &mut impl BufMut,
    pairs: &[KeyValuePair],
) -> anyhow::Result<()> {
    let mut prev = 0u64;
    for pair in pairs {
        let delta = pair
            .key
            .checked_sub(prev)
            .ok_or_else(|| anyhow::anyhow!("key-value pair types must ascend: {}", pair.key))?;
        put_vi64(buf, delta);
        match (&pair.value, pair.key % 2) {
            (KeyValue::Varint(value), 0) => put_vi64(buf, *value),
            (KeyValue::Bytes(value), 1) => {
                anyhow::ensure!(
                    value.len() as u64 <= MAX_KEY_VALUE_LENGTH,
                    "key-value pair value longer than 2^16-1 bytes"
                );
                put_vi64(buf, value.len() as u64);
                buf.put_slice(value);
            }
            _ => anyhow::bail!("key-value pair {} has a value of the wrong kind", pair.key),
        }
        prev = pair.key;
    }
    Ok(())
}

const MAX_REASON_PHRASE_LENGTH: u64 = 1024;

pub(crate) fn get_reason_phrase(buf: &mut impl Buf) -> Result<String, DecodeError> {
    let len = get_vi64(buf)?;
    if len > MAX_REASON_PHRASE_LENGTH {
        return Err(DecodeError::protocol_violation(
            "reason phrase longer than 1024 bytes",
        ));
    }
    let bytes = take(buf, len as usize)?;
    String::from_utf8(bytes.to_vec())
        .map_err(|_| DecodeError::protocol_violation("reason phrase is not UTF-8"))
}

/// Truncates at a character boundary to the 1024 bytes §1.4.4 allows.
pub(crate) fn put_reason_phrase(buf: &mut impl BufMut, reason: &str) {
    let mut len = reason.len().min(MAX_REASON_PHRASE_LENGTH as usize);
    while !reason.is_char_boundary(len) {
        len -= 1;
    }
    put_vi64(buf, len as u64);
    buf.put_slice(&reason.as_bytes()[..len]);
}

const MAX_NAMESPACE_FIELDS: u64 = 32;
const MAX_FULL_TRACK_NAME_LENGTH: usize = 4096;

/// The model keeps names as UTF-8 strings, so a field that is not UTF-8
/// is rejected even though draft-18 allows any bytes.
fn utf8(bytes: Bytes) -> Result<String, DecodeError> {
    String::from_utf8(bytes.to_vec())
        .map_err(|_| DecodeError::protocol_violation("track name field is not UTF-8"))
}

pub(crate) fn get_track_namespace(buf: &mut impl Buf) -> Result<Vec<String>, DecodeError> {
    let count = get_vi64(buf)?;
    if count > MAX_NAMESPACE_FIELDS {
        return Err(DecodeError::protocol_violation(
            "track namespace has more than 32 fields",
        ));
    }
    let mut fields = Vec::with_capacity(count as usize);
    let mut total = 0usize;
    for _ in 0..count {
        let len = get_vi64(buf)? as usize;
        if len == 0 {
            return Err(DecodeError::protocol_violation(
                "empty track namespace field",
            ));
        }
        total = total.saturating_add(len);
        if total > MAX_FULL_TRACK_NAME_LENGTH {
            return Err(DecodeError::protocol_violation(
                "track namespace longer than 4096 bytes",
            ));
        }
        fields.push(utf8(take(buf, len)?)?);
    }
    Ok(fields)
}

pub(crate) fn put_track_namespace(buf: &mut impl BufMut, namespace: &[String]) {
    put_vi64(buf, namespace.len() as u64);
    for field in namespace {
        put_vi64(buf, field.len() as u64);
        buf.put_slice(field.as_bytes());
    }
}

/// Reads a Track Namespace followed by a Track Name, the Full Track Name of
/// §2.4.1.
pub(crate) fn get_full_track_name(
    buf: &mut impl Buf,
) -> Result<(Vec<String>, String), DecodeError> {
    let namespace = get_track_namespace(buf)?;
    let len = get_vi64(buf)? as usize;
    let namespace_len: usize = namespace.iter().map(String::len).sum();
    if namespace_len.saturating_add(len) > MAX_FULL_TRACK_NAME_LENGTH {
        return Err(DecodeError::protocol_violation(
            "full track name longer than 4096 bytes",
        ));
    }
    let name = utf8(take(buf, len)?)?;
    Ok((namespace, name))
}

pub(crate) fn put_full_track_name(buf: &mut impl BufMut, namespace: &[String], name: &str) {
    put_track_namespace(buf, namespace);
    put_vi64(buf, name.len() as u64);
    buf.put_slice(name.as_bytes());
}

#[cfg(test)]
mod tests {
    use bytes::{Bytes, BytesMut};

    use super::{
        DecodeError, KeyValue, KeyValuePair, get_full_track_name, get_key_value_pairs,
        get_reason_phrase, get_track_namespace, get_vi64, put_full_track_name, put_key_value_pairs,
        put_reason_phrase, put_vi64,
    };
    use crate::modules::moqt::control_plane::constants::TerminationErrorCode;

    fn hex(hex: &str) -> Bytes {
        Bytes::from(
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect::<Vec<_>>(),
        )
    }

    fn violation(error: Result<impl std::fmt::Debug, DecodeError>) -> TerminationErrorCode {
        match error {
            Err(DecodeError::Violation { code, .. }) => code,
            other => panic!("expected a violation, got {other:?}"),
        }
    }

    #[test]
    fn the_example_integer_encodings_decode_to_their_values() {
        // Table 2 of §1.4.1.
        let examples = [
            ("25", 37),
            ("8025", 37),
            ("bbbd", 15_293),
            ("ed7f3e7d", 226_442_877),
            ("faa1a0e403d8", 2_893_212_287_960),
            ("fc8998abc66bc0", 151_288_809_941_952),
            ("fefa318fa8e3ca11", 70_423_237_261_249_041),
            ("ffffffffffffffffff", u64::MAX),
        ];
        for (encoded, value) in examples {
            // Act
            let mut buf = hex(encoded);
            let decoded = get_vi64(&mut buf);

            // Assert
            assert_eq!(decoded, Ok(value), "{encoded}");
            assert!(buf.is_empty(), "{encoded}");
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
        assert_eq!(get_vi64(&mut hex("ed7f3e")), Err(DecodeError::NeedMoreData));
    }

    #[test]
    fn key_value_pairs_round_trip_with_delta_encoded_types() {
        // Arrange
        let pairs = vec![
            KeyValuePair {
                key: 2,
                value: KeyValue::Varint(300),
            },
            KeyValuePair {
                key: 7,
                value: KeyValue::Bytes(Bytes::from_static(b"value")),
            },
            KeyValuePair {
                key: 7,
                value: KeyValue::Bytes(Bytes::new()),
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
                value: KeyValue::Varint(0),
            },
            KeyValuePair {
                key: 2,
                value: KeyValue::Varint(0),
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
    fn a_full_track_name_round_trips() {
        // Arrange
        let namespace = vec!["live".to_string(), "room".to_string()];
        let mut buf = BytesMut::new();

        // Act
        put_full_track_name(&mut buf, &namespace, "video");
        let decoded = get_full_track_name(&mut buf.freeze());

        // Assert
        assert_eq!(decoded, Ok((namespace, "video".to_string())));
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
        put_full_track_name(&mut buf, &namespace, &"t".repeat(97));

        // Act / Assert
        assert_eq!(
            violation(get_full_track_name(&mut buf.freeze())),
            TerminationErrorCode::ProtocolViolation
        );
    }
}
