use std::time::{Duration, SystemTime};

use crate::{open_hours::OpenHours, target_language::TargetLanguage};

/// When a language is translated. `always_on` languages start with the
/// broadcast and run while it lasts, so the first viewer of one does not wait
/// for Gemini's first answer; the others run only while subscribed. All stop
/// outside the open hours, and a subscribed one after `language_time_limit`
/// of continuous running, so a tab left open does not keep a Gemini session
/// running.
pub(crate) struct TranslationPolicy {
    pub(crate) open_hours: Option<OpenHours>,
    pub(crate) language_time_limit: Duration,
    pub(crate) always_on: Vec<&'static TargetLanguage>,
}

impl TranslationPolicy {
    pub(crate) fn is_open(&self) -> bool {
        self.open_hours
            .as_ref()
            .is_none_or(|open_hours| open_hours.contains(SystemTime::now()))
    }
}
