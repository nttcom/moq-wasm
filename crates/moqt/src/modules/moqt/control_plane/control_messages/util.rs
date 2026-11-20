use std::ops::RangeInclusive;

use anyhow::bail;
use bytes::{Buf, BytesMut};

use crate::modules::extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, name_reader};

fn get_varint(buf: &mut impl Buf) -> Option<u64> {
    buf.try_get_varint().ok()
}

pub(super) fn get_reason_phrase(buf: &mut impl Buf) -> anyhow::Result<String> {
    Ok(name_reader::get_reason_phrase(buf, get_varint)?)
}

/// Truncates at a character boundary to the 1024 bytes §1.4.3 allows.
pub(super) fn put_reason_phrase(buf: &mut BytesMut, reason: &str) -> anyhow::Result<()> {
    buf.put_string(name_reader::truncate_reason_phrase(reason))
}

/// A Track Namespace or Track Namespace Prefix: 1 to 32 fields (§2.4.1, §9.28).
const TRACK_NAMESPACE_FIELDS: RangeInclusive<u64> = 1..=name_reader::MAX_TRACK_NAMESPACE_FIELDS;

pub(super) fn get_track_namespace(buf: &mut impl Buf) -> anyhow::Result<Vec<String>> {
    Ok(name_reader::get_track_namespace(
        buf,
        get_varint,
        TRACK_NAMESPACE_FIELDS,
    )?)
}

/// Fails, writing nothing, on a namespace §2.4.1 has its receiver close the
/// session over.
pub(super) fn put_track_namespace(buf: &mut BytesMut, namespace: &[String]) -> anyhow::Result<()> {
    name_reader::check_track_namespace(namespace, TRACK_NAMESPACE_FIELDS)?;
    buf.put_varint(namespace.len() as u64)?;
    for field in namespace {
        buf.put_string(field)?;
    }
    Ok(())
}

pub(super) fn get_full_track_name(buf: &mut impl Buf) -> anyhow::Result<(Vec<String>, String)> {
    let namespace = get_track_namespace(buf)?;
    let name = name_reader::get_track_name(buf, get_varint, &namespace)?;
    Ok((namespace, name))
}

/// Fails, writing nothing, like `put_track_namespace` and on a Full Track
/// Name over 4096 bytes (§2.4.1).
pub(super) fn put_full_track_name(
    buf: &mut BytesMut,
    namespace: &[String],
    name: &str,
) -> anyhow::Result<()> {
    name_reader::check_track_name(namespace, name)?;
    put_track_namespace(buf, namespace)?;
    buf.put_string(name)
}

pub(super) fn u8_to_bool(value: u8) -> anyhow::Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => bail!("Invalid value for bool: {value}"),
    }
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;

    use super::{put_full_track_name, put_track_namespace};

    #[test]
    fn a_namespace_or_full_track_name_its_receiver_rejects_is_not_encoded() {
        // Arrange
        let no_fields: Vec<String> = Vec::new();
        let too_many_fields = vec!["n".to_string(); 33];
        let too_long = vec!["n".repeat(4097)];
        let mut buf = BytesMut::new();

        // Act
        let results = [
            put_track_namespace(&mut buf, &no_fields),
            put_track_namespace(&mut buf, &too_many_fields),
            put_track_namespace(&mut buf, &too_long),
            put_full_track_name(&mut buf, &["n".repeat(4000)], &"t".repeat(97)),
        ];

        // Assert
        assert!(results.iter().all(Result::is_err), "{results:?}");
        assert!(buf.is_empty());
    }
}
