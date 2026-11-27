use std::time::Duration;

pub(crate) const TEXT_TRACK_PREFIX: &str = "text_";
pub(crate) const SOURCE_TEXT_TRACK: &str = "text_ja";
const TRANSLATED_TRACK_PREFIX: &str = "audio_";

#[derive(PartialEq, Eq, Hash)]
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

    pub(crate) fn text_track_name(&self) -> String {
        format!("{TEXT_TRACK_PREFIX}{}", self.code)
    }
}

pub(crate) fn find_by_code(code: &str) -> Option<&'static TargetLanguage> {
    TARGET_LANGUAGES
        .iter()
        .find(|language| language.code == code)
}
