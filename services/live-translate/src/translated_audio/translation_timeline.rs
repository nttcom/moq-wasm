use std::time::Duration;

pub(crate) const FRAME_DURATION: Duration = Duration::from_millis(20);

pub(crate) struct TranslationTimeline {
    input_cursor_micros: Option<u64>,
    translation_cursor_micros: Option<u64>,
    gemini_delay_micros: u64,
}

impl TranslationTimeline {
    pub(crate) fn new(gemini_delay: Duration) -> Self {
        Self {
            input_cursor_micros: None,
            translation_cursor_micros: None,
            gemini_delay_micros: duration_micros(gemini_delay),
        }
    }

    pub(crate) fn record_input(&mut self, timestamp_micros: u64) {
        self.input_cursor_micros = Some(timestamp_micros);
    }

    pub(crate) fn restart(&mut self) {
        self.translation_cursor_micros = None;
    }

    pub(crate) fn next_frame_timestamp_micros(&mut self) -> Option<u64> {
        let cursor_micros = self
            .translation_cursor_micros
            .or(self.input_cursor_micros)?;
        self.translation_cursor_micros = Some(cursor_micros + duration_micros(FRAME_DURATION));
        Some(cursor_micros.saturating_sub(self.gemini_delay_micros))
    }
}

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEMINI_DELAY: Duration = Duration::from_millis(3_700);

    #[test]
    fn first_frame_sits_one_gemini_delay_before_the_latest_input() {
        // Arrange
        let mut timeline = TranslationTimeline::new(GEMINI_DELAY);
        timeline.record_input(100_000_000);
        timeline.record_input(106_700_000);

        // Act
        let timestamp_micros = timeline.next_frame_timestamp_micros();

        // Assert
        assert_eq!(timestamp_micros, Some(103_000_000));
    }

    #[test]
    fn later_frames_follow_the_previous_one_whatever_the_input_does() {
        // Arrange
        let mut timeline = TranslationTimeline::new(GEMINI_DELAY);
        timeline.record_input(106_700_000);
        timeline.next_frame_timestamp_micros();
        timeline.record_input(110_000_000);

        // Act
        let second = timeline.next_frame_timestamp_micros();

        // Assert
        assert_eq!(second, Some(103_020_000));
    }

    #[test]
    fn restart_places_the_next_frame_at_the_latest_input_again() {
        // Arrange
        let mut timeline = TranslationTimeline::new(GEMINI_DELAY);
        timeline.record_input(106_700_000);
        timeline.next_frame_timestamp_micros();
        timeline.record_input(115_700_000);

        // Act
        timeline.restart();
        let after_restart = timeline.next_frame_timestamp_micros();

        // Assert
        assert_eq!(after_restart, Some(112_000_000));
    }

    #[test]
    fn frame_before_any_input_has_no_timestamp() {
        // Arrange
        let mut timeline = TranslationTimeline::new(GEMINI_DELAY);

        // Act / Assert
        assert_eq!(timeline.next_frame_timestamp_micros(), None);
    }
}
