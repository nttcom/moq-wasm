use anyhow::{Context, Result};
use media_streaming_format::MediaTimelineRecord;

/// The relay drops cached objects older than `RELAY_CACHE_TTL_SECS` (30 s by
/// default), so a record past that window names a location a subscriber can no
/// longer FETCH.
const RETENTION_US: u64 = 30_000_000;
const MICROS_PER_MILLI: u64 = 1_000;

pub(crate) struct MediaTimeline {
    records: Vec<MediaTimelineRecord>,
    expected_group_id: Option<u64>,
}

impl MediaTimeline {
    pub(crate) fn new() -> Self {
        Self {
            records: Vec::new(),
            expected_group_id: None,
        }
    }

    pub(crate) fn record(&mut self, group_id: u64, presentation_us: u64, encoded_at_ms: u64) {
        if self
            .expected_group_id
            .is_some_and(|expected| expected != group_id)
        {
            self.records.clear();
        }
        self.expected_group_id = Some(group_id + 1);
        self.records.push(MediaTimelineRecord::new(
            millis_from_micros(presentation_us),
            group_id,
            0,
            encoded_at_ms,
        ));
        self.drop_records_beyond_relay_retention(presentation_us);
    }

    pub(crate) fn document(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(&self.records).context("serialize msf media timeline")
    }

    fn drop_records_beyond_relay_retention(&mut self, newest_presentation_us: u64) {
        let horizon = millis_from_micros(newest_presentation_us.saturating_sub(RETENTION_US));
        self.records
            .retain(|record| record.presentation_time_ms() >= horizon);
    }
}

fn millis_from_micros(micros: u64) -> u64 {
    (micros + MICROS_PER_MILLI / 2) / MICROS_PER_MILLI
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_one_entry_per_group_at_object_zero() {
        // Arrange
        let mut timeline = MediaTimeline::new();

        // Act
        timeline.record(700, 0, 1_759_924_158_381);
        timeline.record(701, 2_002_400, 1_759_924_160_383);

        // Assert
        assert_eq!(
            timeline.document().unwrap(),
            br#"[[0,[700,0],1759924158381],[2002,[701,0],1759924160383]]"#
        );
    }

    #[test]
    fn drops_records_the_relay_no_longer_caches() {
        // Arrange
        let mut timeline = MediaTimeline::new();
        timeline.record(1, 0, 10);
        timeline.record(2, 2_000_000, 12);

        // Act: the newest record sits 31 s past the first one
        timeline.record(3, 31_000_000, 43);

        // Assert
        assert_eq!(
            timeline.document().unwrap(),
            br#"[[2000,[2,0],12],[31000,[3,0],43]]"#
        );
    }

    #[test]
    fn clears_the_timeline_when_group_numbering_restarts() {
        // Arrange
        let mut timeline = MediaTimeline::new();
        timeline.record(1, 0, 10);
        timeline.record(2, 2_000_000, 12);

        // Act: a resubscribe reseeds the group id, breaking the §6.1 sequence
        timeline.record(9_000, 4_000_000, 14);

        // Assert
        assert_eq!(timeline.document().unwrap(), br#"[[4000,[9000,0],14]]"#);
    }
}
