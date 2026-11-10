use std::{
    fmt,
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Weekly opening hours at a fixed UTC offset, such as `Mon-Fri 09:00-17:30
/// +09:00`: the notation of the AI demo bots' `--open-hours`. The weekday
/// range and the time range do not wrap around.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenHours {
    first_weekday: u8,
    last_weekday: u8,
    opens_minute: u16,
    closes_minute: u16,
    offset_seconds: i64,
    text: String,
}

impl OpenHours {
    pub(crate) fn contains(&self, moment: SystemTime) -> bool {
        let unix_seconds = match moment.duration_since(UNIX_EPOCH) {
            Ok(elapsed) => elapsed.as_secs() as i64,
            Err(error) => -(error.duration().as_secs() as i64),
        };
        let local_seconds = unix_seconds + self.offset_seconds;
        let days = local_seconds.div_euclid(86_400);
        // 1970-01-01 was a Thursday, index 3 from Monday.
        let weekday = (days + 3).rem_euclid(7) as u8;
        let minute = (local_seconds.rem_euclid(86_400) / 60) as u16;
        (self.first_weekday..=self.last_weekday).contains(&weekday)
            && (self.opens_minute..self.closes_minute).contains(&minute)
    }
}

impl FromStr for OpenHours {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> Result<Self> {
        let malformed =
            || format!("expected hours like 'Mon-Fri 09:00-17:30 +09:00', got {text:?}");
        let [days, hours, offset] = text.split_whitespace().collect::<Vec<_>>()[..] else {
            bail!(malformed());
        };
        let (first, last) = days.split_once('-').with_context(malformed)?;
        let first_weekday = weekday_index(first).with_context(malformed)?;
        let last_weekday = weekday_index(last).with_context(malformed)?;
        let (opens, closes) = hours.split_once('-').with_context(malformed)?;
        let opens_minute = parse_minute(opens).with_context(malformed)?;
        let closes_minute = parse_minute(closes).with_context(malformed)?;
        if first_weekday > last_weekday || opens_minute >= closes_minute {
            bail!("opening hours must not wrap around: {text:?}");
        }
        let (sign, offset_hm) = offset.split_at_checked(1).with_context(malformed)?;
        let offset_seconds = i64::from(parse_minute(offset_hm).with_context(malformed)?) * 60;
        let offset_seconds = match sign {
            "+" => offset_seconds,
            "-" => -offset_seconds,
            _ => bail!(malformed()),
        };
        Ok(Self {
            first_weekday,
            last_weekday,
            opens_minute,
            closes_minute,
            offset_seconds,
            text: text.trim().to_string(),
        })
    }
}

impl fmt::Display for OpenHours {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

fn weekday_index(name: &str) -> Option<u8> {
    WEEKDAYS
        .iter()
        .position(|weekday| *weekday == name)
        .map(|index| index as u8)
}

/// `HH:MM` with both parts of two digits, as the bots require, as minutes
/// since midnight.
fn parse_minute(text: &str) -> Option<u16> {
    let (hours, minutes) = text.split_once(':')?;
    if hours.len() != 2 || minutes.len() != 2 {
        return None;
    }
    let hours: u16 = hours.parse().ok()?;
    let minutes: u16 = minutes.parse().ok()?;
    (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn weekday_hours() -> OpenHours {
        "Mon-Fri 09:00-17:30 +09:00".parse().expect("valid hours")
    }

    #[test]
    fn contains_compares_the_local_time_at_the_offset() {
        // Arrange: UNIX seconds; 2026-10-18 is a Sunday; the last case, 00:30 UTC, is 09:30 at +09:00
        let cases = [
            (1_792_368_000, true),  // 2026-10-19 09:00 +09:00
            (1_792_398_599, true),  // 2026-10-19 17:29:59 +09:00
            (1_792_398_600, false), // 2026-10-19 17:30 +09:00
            (1_792_367_999, false), // 2026-10-19 08:59:59 +09:00
            (1_792_292_400, false), // 2026-10-18 12:00 +09:00
            (1_792_369_800, true),  // 2026-10-19 00:30 +00:00
        ];

        // Act / Assert
        for (unix_seconds, expected) in cases {
            let moment = UNIX_EPOCH + Duration::from_secs(unix_seconds);
            assert_eq!(weekday_hours().contains(moment), expected, "{unix_seconds}");
        }
    }

    #[test]
    fn parse_rejects_malformed_or_wrapping_hours() {
        // Act / Assert
        for text in [
            "Mon-Fri 9:00-17:30 +09:00",
            "Mon-Fri 09:00-17:30",
            "Fri-Mon 09:00-17:30 +09:00",
            "Mon-Fri 18:00-09:00 +09:00",
        ] {
            assert!(text.parse::<OpenHours>().is_err(), "{text}");
        }
    }

    #[test]
    fn displays_the_text_it_was_parsed_from() {
        // Act / Assert
        assert_eq!(weekday_hours().to_string(), "Mon-Fri 09:00-17:30 +09:00");
    }
}
