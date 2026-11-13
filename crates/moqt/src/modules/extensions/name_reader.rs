//! Readers of the Reason Phrase, Track Namespace and Track Name with the
//! limits draft-14 (§1.4.3, §2.4.1) and draft-18 (§1.4.4, §2.4.1) share.

use std::ops::RangeInclusive;

use bytes::{Buf, Bytes};
use thiserror::Error;

pub(crate) const MAX_REASON_PHRASE_LENGTH: u64 = 1024;
const MAX_FULL_TRACK_NAME_LENGTH: usize = 4096;

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

/// `fields` is the number of fields the draft allows.
pub(crate) fn get_track_namespace<B: Buf>(
    buf: &mut B,
    get: GetInt<B>,
    fields: RangeInclusive<u64>,
) -> Result<Vec<String>, NameError> {
    let count = get_int(buf, get)?;
    if !fields.contains(&count) {
        return Err(NameError::ProtocolViolation(
            "track namespace field count out of range",
        ));
    }
    let mut namespace = Vec::with_capacity(count as usize);
    let mut total = 0usize;
    for _ in 0..count {
        let len = get_int(buf, get)? as usize;
        total = total.saturating_add(len);
        if total > MAX_FULL_TRACK_NAME_LENGTH {
            return Err(NameError::ProtocolViolation(
                "track namespace longer than 4096 bytes",
            ));
        }
        namespace.push(utf8(take(buf, len)?)?);
    }
    Ok(namespace)
}

/// Reads the Track Name that follows `namespace` in a Full Track Name.
pub(crate) fn get_track_name<B: Buf>(
    buf: &mut B,
    get: GetInt<B>,
    namespace: &[String],
) -> Result<String, NameError> {
    let len = get_int(buf, get)? as usize;
    let namespace_len: usize = namespace.iter().map(String::len).sum();
    if namespace_len.saturating_add(len) > MAX_FULL_TRACK_NAME_LENGTH {
        return Err(NameError::ProtocolViolation(
            "full track name longer than 4096 bytes",
        ));
    }
    utf8(take(buf, len)?)
}
