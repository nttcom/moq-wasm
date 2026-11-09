use std::{fmt, str::FromStr};

use anyhow::{Context, Result, bail};
use time::{OffsetDateTime, Time, UtcOffset};

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Weekly opening hours at a fixed UTC offset, such as `Mon-Fri 09:00-17:30
/// +09:00`: the notation of the AI demo bots' `--open-hours`. The weekday
/// range and the time range do not wrap around.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenHours {
    first_weekday: u8,
    last_weekday: u8,
    opens: Time,
    closes: Time,
    offset: UtcOffset,
    text: String,
}

impl OpenHours {
    pub(crate) fn contains(&self, moment: OffsetDateTime) -> bool {
        let local = moment.to_offset(self.offset);
        let weekday = local.weekday().number_days_from_monday();
        (self.first_weekday..=self.last_weekday).contains(&weekday)
            && self.opens <= local.time()
            && local.time() < self.closes
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
        let opens = parse_time(opens).with_context(malformed)?;
        let closes = parse_time(closes).with_context(malformed)?;
        if first_weekday > last_weekday || opens >= closes {
            bail!("opening hours must not wrap around: {text:?}");
        }
        let (sign, offset_hm) = offset.split_at_checked(1).with_context(malformed)?;
        let offset_time = parse_time(offset_hm).with_context(malformed)?;
        let minutes = i32::from(offset_time.hour()) * 60 + i32::from(offset_time.minute());
        let minutes = match sign {
            "+" => minutes,
            "-" => -minutes,
            _ => bail!(malformed()),
        };
        let offset = UtcOffset::from_whole_seconds(minutes * 60).with_context(malformed)?;
        Ok(Self {
            first_weekday,
            last_weekday,
            opens,
            closes,
            offset,
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

/// `HH:MM` with both parts of two digits, as the bots require.
fn parse_time(text: &str) -> Option<Time> {
    let (hours, minutes) = text.split_once(':')?;
    if hours.len() != 2 || minutes.len() != 2 {
        return None;
    }
    Time::from_hms(hours.parse().ok()?, minutes.parse().ok()?, 0).ok()
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn weekday_hours() -> OpenHours {
        "Mon-Fri 09:00-17:30 +09:00".parse().expect("valid hours")
    }

    #[test]
    fn contains_compares_the_local_time_at_the_offset() {
        // Arrange: 2026-10-18 is a Sunday; the last case, 00:30 UTC, is 09:30 at +09:00
        let cases = [
            (datetime!(2026-10-19 09:00 +09:00), true),
            (datetime!(2026-10-19 17:29:59 +09:00), true),
            (datetime!(2026-10-19 17:30 +09:00), false),
            (datetime!(2026-10-19 08:59:59 +09:00), false),
            (datetime!(2026-10-18 12:00 +09:00), false),
            (datetime!(2026-10-19 00:30 +00:00), true),
        ];

        // Act / Assert
        for (moment, expected) in cases {
            assert_eq!(weekday_hours().contains(moment), expected, "{moment}");
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
