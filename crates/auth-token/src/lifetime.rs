use std::time::Duration;

use anyhow::anyhow;

/// Reads a lifetime such as `15s`, `30m`, `12h` or `365d`. Fails without one of those units, on a
/// count that is not a whole number, and on one too large for a `Duration`.
pub fn parse_lifetime(text: &str) -> anyhow::Result<Duration> {
    let invalid = || anyhow!("invalid lifetime {text:?}, expected e.g. 30m, 12h, 365d");
    let unit_seconds = match text.as_bytes().last() {
        Some(b's') => 1,
        Some(b'm') => 60,
        Some(b'h') => 3600,
        Some(b'd') => 86400,
        _ => return Err(invalid()),
    };
    let count = text[..text.len() - 1]
        .parse::<u64>()
        .map_err(|_| invalid())?;
    count
        .checked_mul(unit_seconds)
        .map(Duration::from_secs)
        .ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_unit() {
        // Act / Assert
        assert_eq!(parse_lifetime("15s").unwrap(), Duration::from_secs(15));
        assert_eq!(parse_lifetime("30m").unwrap(), Duration::from_secs(30 * 60));
        assert_eq!(
            parse_lifetime("12h").unwrap(),
            Duration::from_secs(12 * 3600)
        );
        assert_eq!(
            parse_lifetime("365d").unwrap(),
            Duration::from_secs(365 * 86400)
        );
    }

    #[test]
    fn refuses_a_lifetime_without_a_unit_or_a_whole_count() {
        // Act / Assert
        for text in ["12", "h", "1.5h", ""] {
            assert!(parse_lifetime(text).is_err(), "{text}");
        }
    }
}
