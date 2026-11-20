//! Readers of the Reason Phrase, Track Namespace and Track Name with the
//! limits draft-14 (§1.4.3, §2.4.1) and draft-18 (§1.4.4, §2.4.1) share,
//! the cut a Reason Phrase gets before it is written, and the checks that
//! keep a writer from sending a name its receiver closes the session over.

use std::ops::RangeInclusive;

use bytes::{Buf, Bytes};
use thiserror::Error;

pub(crate) const MAX_REASON_PHRASE_LENGTH: u64 = 1024;
/// Lengths are compared as `u64` before the cast, since `as usize` keeps
/// only the low 32 bits on wasm32.
const MAX_FULL_TRACK_NAME_LENGTH: u64 = 4096;
pub(crate) const MAX_TRACK_NAMESPACE_FIELDS: u64 = 32;

const FIELD_COUNT_OUT_OF_RANGE: &str = "track namespace field count out of range";
const NAMESPACE_TOO_LONG: &str = "track namespace longer than 4096 bytes";
const FULL_TRACK_NAME_TOO_LONG: &str = "full track name longer than 4096 bytes";

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(crate) enum NameError {
    #[error("need more data to decode")]
    NeedMoreData,
    #[error("{0}")]
    ProtocolViolation(&'static str),
}

/// A draft's variable-length integer reader; `None` when the buffer ends
/// inside the integer.
type GetInt<B> = fn(&mut B) -> Option<u64>;

fn get_int<B: Buf>(buf: &mut B, get: GetInt<B>) -> Result<u64, NameError> {
    get(buf).ok_or(NameError::NeedMoreData)
}

fn take(buf: &mut impl Buf, len: usize) -> Result<Bytes, NameError> {
    if buf.remaining() < len {
        return Err(NameError::NeedMoreData);
    }
    Ok(buf.copy_to_bytes(len))
}

/// The model keeps names as UTF-8 strings, so a field that is not UTF-8
/// is rejected even though both drafts allow any bytes.
fn utf8(bytes: Bytes) -> Result<String, NameError> {
    String::from_utf8(bytes.to_vec())
        .map_err(|_| NameError::ProtocolViolation("track name field is not UTF-8"))
}

pub(crate) fn get_reason_phrase<B: Buf>(buf: &mut B, get: GetInt<B>) -> Result<String, NameError> {
    let len = get_int(buf, get)?;
    if len > MAX_REASON_PHRASE_LENGTH {
        return Err(NameError::ProtocolViolation(
            "reason phrase longer than 1024 bytes",
        ));
    }
    Ok(String::from_utf8_lossy(&take(buf, len as usize)?).into_owned())
}

/// Cuts `reason` at a character boundary to the 1024 bytes a receiver accepts.
pub(crate) fn truncate_reason_phrase(reason: &str) -> &str {
    let mut len = reason.len().min(MAX_REASON_PHRASE_LENGTH as usize);
    while !reason.is_char_boundary(len) {
        len -= 1;
    }
    &reason[..len]
}

/// `fields` is the number of fields the draft allows.
pub(crate) fn get_track_namespace<B: Buf>(
    buf: &mut B,
    get: GetInt<B>,
    fields: RangeInclusive<u64>,
) -> Result<Vec<String>, NameError> {
    let count = get_int(buf, get)?;
    if !fields.contains(&count) {
        return Err(NameError::ProtocolViolation(FIELD_COUNT_OUT_OF_RANGE));
    }
    let mut namespace = Vec::with_capacity(count as usize);
    let mut total = 0u64;
    for _ in 0..count {
        let len = get_int(buf, get)?;
        total = total.saturating_add(len);
        if total > MAX_FULL_TRACK_NAME_LENGTH {
            return Err(NameError::ProtocolViolation(NAMESPACE_TOO_LONG));
        }
        namespace.push(utf8(take(buf, len as usize)?)?);
    }
    Ok(namespace)
}

/// Reads the Track Name that follows `namespace` in a Full Track Name.
pub(crate) fn get_track_name<B: Buf>(
    buf: &mut B,
    get: GetInt<B>,
    namespace: &[String],
) -> Result<String, NameError> {
    let len = get_int(buf, get)?;
    if byte_length(namespace).saturating_add(len) > MAX_FULL_TRACK_NAME_LENGTH {
        return Err(NameError::ProtocolViolation(FULL_TRACK_NAME_TOO_LONG));
    }
    utf8(take(buf, len as usize)?)
}

/// Fails on a namespace `get_track_namespace` rejects with the same
/// `fields`: a field count outside it, or more than 4096 bytes in all.
pub(crate) fn check_track_namespace(
    namespace: &[String],
    fields: RangeInclusive<u64>,
) -> Result<(), NameError> {
    if !fields.contains(&(namespace.len() as u64)) {
        return Err(NameError::ProtocolViolation(FIELD_COUNT_OUT_OF_RANGE));
    }
    if byte_length(namespace) > MAX_FULL_TRACK_NAME_LENGTH {
        return Err(NameError::ProtocolViolation(NAMESPACE_TOO_LONG));
    }
    Ok(())
}

/// Fails on a Full Track Name `get_track_name` rejects: more than 4096
/// bytes in the namespace's fields and `name` together.
pub(crate) fn check_track_name(namespace: &[String], name: &str) -> Result<(), NameError> {
    if byte_length(namespace).saturating_add(name.len() as u64) > MAX_FULL_TRACK_NAME_LENGTH {
        return Err(NameError::ProtocolViolation(FULL_TRACK_NAME_TOO_LONG));
    }
    Ok(())
}

fn byte_length(namespace: &[String]) -> u64 {
    namespace.iter().map(|field| field.len() as u64).sum()
}

#[cfg(test)]
mod tests {
    use bytes::{Buf, BufMut, BytesMut};

    use super::{
        NameError, check_track_name, check_track_namespace, get_track_name, get_track_namespace,
    };
    use crate::modules::extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt};

    fn get_varint(buf: &mut impl Buf) -> Option<u64> {
        buf.try_get_varint().ok()
    }

    /// `count` fields of `total` bytes in all, the first taking the remainder.
    fn namespace_of(count: usize, total: usize) -> Vec<String> {
        (0..count)
            .map(|index| {
                let share = total / count;
                let len = if index == 0 {
                    total - share * (count - 1)
                } else {
                    share
                };
                "n".repeat(len)
            })
            .collect()
    }

    #[test]
    fn a_namespace_of_32_fields_passes_and_33_fails() {
        // Act / Assert
        assert_eq!(check_track_namespace(&namespace_of(32, 32), 1..=32), Ok(()));
        assert!(check_track_namespace(&namespace_of(33, 33), 1..=32).is_err());
    }

    #[test]
    fn a_namespace_of_4096_bytes_passes_and_4097_fails() {
        // Act / Assert
        assert_eq!(
            check_track_namespace(&namespace_of(2, 4096), 1..=32),
            Ok(())
        );
        assert!(check_track_namespace(&namespace_of(2, 4097), 1..=32).is_err());
    }

    #[test]
    fn a_full_track_name_of_4096_bytes_passes_and_4097_fails() {
        // Arrange
        let namespace = namespace_of(2, 4000);

        // Act / Assert
        assert_eq!(check_track_name(&namespace, &"t".repeat(96)), Ok(()));
        assert!(check_track_name(&namespace, &"t".repeat(97)).is_err());
    }

    /// Its low 32 bits are 1, so a 32-bit target that truncates it reads a
    /// 1-byte field.
    const LENGTH_OVER_U32: u64 = (1 << 32) + 1;

    #[test]
    fn a_namespace_field_length_over_u32_is_a_protocol_violation() {
        // Arrange
        let mut buf = BytesMut::new();
        buf.put_varint(1).unwrap();
        buf.put_varint(LENGTH_OVER_U32).unwrap();
        buf.put_u8(b'n');

        // Act
        let result = get_track_namespace(&mut buf.freeze(), get_varint, 1..=32);

        // Assert
        assert!(
            matches!(result, Err(NameError::ProtocolViolation(_))),
            "{result:?}"
        );
    }

    #[test]
    fn a_track_name_length_over_u32_is_a_protocol_violation() {
        // Arrange
        let mut buf = BytesMut::new();
        buf.put_varint(LENGTH_OVER_U32).unwrap();
        buf.put_u8(b't');

        // Act
        let result = get_track_name(&mut buf.freeze(), get_varint, &["n".to_string()]);

        // Assert
        assert!(
            matches!(result, Err(NameError::ProtocolViolation(_))),
            "{result:?}"
        );
    }

    mod properties {
        use std::ops::RangeInclusive;

        use bytes::BytesMut;
        use proptest::prelude::*;

        use super::super::{
            MAX_REASON_PHRASE_LENGTH, check_track_name, check_track_namespace, get_track_name,
            get_track_namespace, truncate_reason_phrase,
        };
        use super::{get_varint, namespace_of};
        use crate::modules::extensions::buf_put_ext::BufPutExt;
        use crate::modules::test_support::control_message_strategies::reason_phrase;

        fn byte_count() -> impl Strategy<Value = usize> {
            prop_oneof![4090..=4100usize, 0..=4200usize]
        }

        /// Up to 34 fields, past the 32 either draft allows.
        fn namespace() -> impl Strategy<Value = Vec<String>> {
            (0..=34usize, byte_count()).prop_map(|(count, total)| namespace_of(count, total))
        }

        /// The field counts of draft-14 and draft-18.
        fn fields() -> impl Strategy<Value = RangeInclusive<u64>> {
            prop_oneof![Just(1..=32), Just(0..=32)]
        }

        fn written_namespace(namespace: &[String]) -> BytesMut {
            let mut buf = BytesMut::new();
            buf.put_varint(namespace.len() as u64).unwrap();
            for field in namespace {
                buf.put_string(field).unwrap();
            }
            buf
        }

        proptest! {
            #[test]
            fn a_namespace_passes_its_check_exactly_when_its_reader_accepts_it(
                namespace in namespace(),
                fields in fields(),
            ) {
                // Arrange
                let mut buf = written_namespace(&namespace).freeze();

                // Act
                let checked = check_track_namespace(&namespace, fields.clone());
                let read = get_track_namespace(&mut buf, get_varint, fields);

                // Assert
                prop_assert_eq!(checked.is_ok(), read.is_ok(), "{:?} / {:?}", checked, read);
            }

            #[test]
            fn a_full_track_name_passes_its_check_exactly_when_its_reader_accepts_it(
                namespace in namespace(),
                name_length in byte_count(),
            ) {
                // Arrange
                let name = "t".repeat(name_length);
                let mut buf = BytesMut::new();
                buf.put_string(&name).unwrap();

                // Act
                let checked = check_track_name(&namespace, &name);
                let read = get_track_name(&mut buf.freeze(), get_varint, &namespace);

                // Assert
                prop_assert_eq!(checked.is_ok(), read.is_ok(), "{:?} / {:?}", checked, read);
            }

            #[test]
            fn a_reason_phrase_is_cut_to_its_longest_prefix_within_1024_bytes(
                reason in reason_phrase(),
            ) {
                // Act
                let cut = truncate_reason_phrase(&reason);

                // Assert
                let limit = MAX_REASON_PHRASE_LENGTH as usize;
                prop_assert!(reason.starts_with(cut));
                prop_assert!(cut.len() <= limit);
                let next = reason[cut.len()..].chars().next();
                prop_assert!(next.is_none_or(|next| cut.len() + next.len_utf8() > limit));
            }
        }
    }
}
