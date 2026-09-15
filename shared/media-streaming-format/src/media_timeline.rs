use serde::{Deserialize, Serialize};

/// One media timeline record in the explicit entry format of
/// draft-ietf-moq-msf-01 §7.1.1. The ordinal position
/// of each item defines its type: the media presentation timestamp in
/// milliseconds, the MOQT Location as `[group id, object id]`, and the wallclock
/// time at which the media was encoded in milliseconds since the Unix epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaTimelineRecord(u64, (u64, u64), u64);

impl MediaTimelineRecord {
    pub fn new(
        presentation_time_ms: u64,
        group_id: u64,
        object_id: u64,
        encoded_at_ms: u64,
    ) -> Self {
        Self(presentation_time_ms, (group_id, object_id), encoded_at_ms)
    }

    pub fn presentation_time_ms(&self) -> u64 {
        self.0
    }

    pub fn group_id(&self) -> u64 {
        self.1.0
    }

    pub fn object_id(&self) -> u64 {
        self.1.1
    }

    pub fn encoded_at_ms(&self) -> u64 {
        self.2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_as_the_three_item_array_of_the_draft_example() {
        // Arrange
        let record = MediaTimelineRecord::new(2002, 1, 0, 1_759_924_160_383);

        // Act
        let json = serde_json::to_string(&record).unwrap();

        // Assert
        assert_eq!(json, "[2002,[1,0],1759924160383]");
    }

    #[test]
    fn deserializes_the_draft_example_document() {
        // Arrange
        let document = "[[0,[0,0],1759924158381],[2002,[1,0],1759924160383]]";

        // Act
        let records: Vec<MediaTimelineRecord> = serde_json::from_str(document).unwrap();

        // Assert
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].presentation_time_ms(), 2002);
        assert_eq!(records[1].group_id(), 1);
        assert_eq!(records[1].object_id(), 0);
        assert_eq!(records[1].encoded_at_ms(), 1_759_924_160_383);
    }
}
