use anyhow::bail;
use bytes::Buf;

use crate::modules::extensions::{buf_get_ext::BufGetExt, name_reader};

fn get_varint(buf: &mut impl Buf) -> Option<u64> {
    buf.try_get_varint().ok()
}

pub(super) fn get_reason_phrase(buf: &mut impl Buf) -> anyhow::Result<String> {
    Ok(name_reader::get_reason_phrase(buf, get_varint)?)
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
