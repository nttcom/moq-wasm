use std::time::Duration;

const TRANSLATED_TRACK_PREFIX: &str = "audio_";

pub(crate) struct TargetLanguage {
    pub(crate) code: &'static str,
    pub(crate) gemini_delay: Duration,
}

pub(crate) const TARGET_LANGUAGES: [TargetLanguage; 5] = [
    TargetLanguage {
        code: "en",
        gemini_delay: Duration::from_millis(3_700),
    },
    TargetLanguage {
        code: "es",
        gemini_delay: Duration::from_millis(3_700),
    },
    TargetLanguage {
        code: "de",
        gemini_delay: Duration::from_millis(3_700),
    },
    TargetLanguage {
        code: "ko",
        gemini_delay: Duration::from_millis(3_700),
    },
    TargetLanguage {
        code: "zh-Hans",
        gemini_delay: Duration::from_millis(3_700),
    },
];

impl TargetLanguage {
    pub(crate) fn track_name(&self) -> String {
        format!("{TRANSLATED_TRACK_PREFIX}{}", self.code)
    }
}

pub(crate) fn find_by_track_name(track_name: &str) -> Option<&'static TargetLanguage> {
    find_by_code(track_name.strip_prefix(TRANSLATED_TRACK_PREFIX)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_name_round_trips_to_the_language() {
        // Arrange
        let korean = &TARGET_LANGUAGES[3];

        // Act
        let found = find_by_track_name(&korean.track_name());

        // Assert
        assert_eq!(found.map(|language| language.code), Some("ko"));
    }

    #[test]
    fn unknown_language_and_foreign_track_names_are_not_found() {
        // Act / Assert
        assert!(find_by_track_name("audio_fr").is_none());
        assert!(find_by_track_name("catalog").is_none());
        assert!(find_by_track_name("audio_128kbps").is_none());
    }
}
