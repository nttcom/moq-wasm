use anyhow::bail;

pub(super) fn u8_to_bool(value: u8) -> anyhow::Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => bail!("Invalid value for bool: {value}"),
    }
}
