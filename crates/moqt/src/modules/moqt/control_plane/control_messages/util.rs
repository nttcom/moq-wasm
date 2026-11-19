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
pub(super) fn put_reason_phrase(buf: &mut BytesMut, reason: &str) {
    buf.put_string(name_reader::truncate_reason_phrase(reason));
}

/// A Track Namespace or Track Namespace Prefix: 1 to 32 fields (§2.4.1, §9.28).
pub(super) fn get_track_namespace(buf: &mut impl Buf) -> anyhow::Result<Vec<String>> {
    Ok(name_reader::get_track_namespace(buf, get_varint, 1..=32)?)
}

pub(super) fn get_full_track_name(buf: &mut impl Buf) -> anyhow::Result<(Vec<String>, String)> {
    let namespace = get_track_namespace(buf)?;
    let name = name_reader::get_track_name(buf, get_varint, &namespace)?;
    Ok((namespace, name))
}

pub(super) fn u8_to_bool(value: u8) -> anyhow::Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => bail!("Invalid value for bool: {value}"),
    }
}
