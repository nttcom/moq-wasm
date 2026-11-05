use std::time::Duration;

use crate::open_hours::OpenHours;

/// What bounds the Gemini cost besides the session cap: a language is
/// translated only inside the open hours, and for at most
/// `language_time_limit` at a stretch, so a tab left open does not keep a
/// Gemini session running.
pub(crate) struct TranslationLimits {
    pub(crate) open_hours: Option<OpenHours>,
    pub(crate) language_time_limit: Duration,
}
