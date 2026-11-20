use std::time::Duration;

use crate::target_language::TargetLanguage;

/// When a language is translated. `always_on` languages start with the
/// broadcast and run while it lasts, so the first viewer of one does not wait
/// for Gemini's first answer; the others run only while subscribed, and stop
/// after `language_time_limit` of continuous running, so a tab left open does
/// not keep a Gemini session running.
pub(crate) struct TranslationPolicy {
    pub(crate) language_time_limit: Duration,
    pub(crate) always_on: Vec<&'static TargetLanguage>,
}
