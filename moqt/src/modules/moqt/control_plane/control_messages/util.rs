use anyhow::bail;

pub(super) fn u8_to_bool(value: u8) -> anyhow::Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => {
            tracing::error!("Invalid value for bool: {}", value);
            bail!("Invalid value for bool")
        }
    }
}
