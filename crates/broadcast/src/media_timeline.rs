use anyhow::{Context, Result};
use msf::MediaTimelineRecord;

/// The relay drops cached objects older than `RELAY_CACHE_TTL_SECS` (60 s by
/// default), so a record past that window names a location a subscriber can no
/// longer FETCH.
const RETENTION_US: u64 = 60_000_000;
const MICROS_PER_MILLI: u64 = 1_000;

#[derive(Default)]
pub(crate) struct MediaTimeline {
    records: Vec<MediaTimelineRecord>,
}

impl MediaTimeline {
    pub(crate) fn record(&mut self, group_id: u64, presentation_us: u64, encoded_at_ms: u64) {
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

/// draft-ietf-moq-msf-01 §7.1.1 wants the presentation timestamp as the floor in
/// integral milliseconds, not rounded to the nearest one.
fn millis_from_micros(micros: u64) -> u64 {
    micros / MICROS_PER_MILLI
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_one_entry_per_group_at_object_zero() {
        // Arrange
        let mut timeline = MediaTimeline::default();

        // Act
        timeline.record(700, 0, 1_759_924_158_381);
        timeline.record(701, 2_002_900, 1_759_924_160_383);

        // Assert: 2_002_900 micros floors to 2002 rather than rounding to 2003
        assert_eq!(
            timeline.document().unwrap(),
            br#"[[0,[700,0],1759924158381],[2002,[701,0],1759924160383]]"#
        );
    }

    #[test]
    fn drops_records_the_relay_no_longer_caches() {
        // Arrange
        let mut timeline = MediaTimeline::default();
        timeline.record(1, 0, 10);
        timeline.record(2, 2_000_000, 12);

        // Act: the newest record sits 61 s past the first one
        timeline.record(3, 61_000_000, 73);

        // Assert
        assert_eq!(
            timeline.document().unwrap(),
            br#"[[2000,[2,0],12],[61000,[3,0],73]]"#
        );
    }

    #[test]
    fn keeps_the_records_across_a_gap_in_the_group_ids() {
        // Arrange
        let mut timeline = MediaTimeline::default();
        timeline.record(1, 0, 10);

        // Act
        timeline.record(5, 2_000_000, 12);

        // Assert
        assert_eq!(
            timeline.document().unwrap(),
            br#"[[0,[1,0],10],[2000,[5,0],12]]"#
        );
    }
}
