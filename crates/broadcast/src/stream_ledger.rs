use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const FINISHED_RETENTION: Duration = Duration::from_secs(5);

/// Subgroup streams this publisher has opened, kept for a few seconds after
/// they are finished so a viewer of the ledger sees groups end as well as
/// start. Whether QUIC has delivered a finished stream is not observable here.
#[derive(Clone, Default)]
pub struct StreamLedger {
    records: Arc<Mutex<Vec<StreamRecord>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamRecord {
    pub track_name: String,
    pub group_id: u64,
    pub opened_at: Instant,
    pub bytes: u64,
    pub objects: u64,
    pub finished_at: Option<Instant>,
}

impl StreamRecord {
    pub fn is_open(&self) -> bool {
        self.finished_at.is_none()
    }
}

impl StreamLedger {
    pub(crate) fn open(&self, track_name: &str, group_id: u64) {
        let now = Instant::now();
        let mut records = self.records.lock().unwrap();
        for record in records.iter_mut() {
            if record.track_name == track_name && record.is_open() {
                record.finished_at = Some(now);
            }
        }
        records.push(StreamRecord {
            track_name: track_name.to_string(),
            group_id,
            opened_at: now,
            bytes: 0,
            objects: 0,
            finished_at: None,
        });
    }

    pub(crate) fn add_object(&self, track_name: &str, bytes: usize) {
        let mut records = self.records.lock().unwrap();
        if let Some(record) = records
            .iter_mut()
            .find(|record| record.track_name == track_name && record.is_open())
        {
            record.bytes += bytes as u64;
            record.objects += 1;
        }
    }

    pub(crate) fn finish(&self, track_name: &str) {
        let now = Instant::now();
        let mut records = self.records.lock().unwrap();
        for record in records.iter_mut() {
            if record.track_name == track_name && record.is_open() {
                record.finished_at = Some(now);
            }
        }
    }

    pub fn snapshot(&self) -> Vec<StreamRecord> {
        let now = Instant::now();
        let mut records = self.records.lock().unwrap();
        records.retain(|record| {
            record
                .finished_at
                .is_none_or(|finished_at| now.duration_since(finished_at) < FINISHED_RETENTION)
        });
        records.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_the_next_group_finishes_the_previous_one_of_the_same_track() {
        // Arrange
        let ledger = StreamLedger::default();
        ledger.open("video", 1);
        ledger.add_object("video", 100);
        ledger.open("audio", 1);

        // Act
        ledger.open("video", 2);
        ledger.add_object("video", 5);

        // Assert
        let records = ledger.snapshot();
        let video: Vec<_> = records.iter().filter(|r| r.track_name == "video").collect();
        assert_eq!(video.len(), 2);
        assert!(!video[0].is_open());
        assert_eq!((video[0].bytes, video[0].objects), (100, 1));
        assert!(video[1].is_open());
        assert_eq!((video[1].bytes, video[1].objects), (5, 1));
        assert!(
            records
                .iter()
                .any(|r| r.track_name == "audio" && r.is_open())
        );
    }

    #[test]
    fn finish_closes_the_open_group_and_later_objects_are_not_counted() {
        // Arrange
        let ledger = StreamLedger::default();
        ledger.open("video", 1);

        // Act
        ledger.finish("video");
        ledger.add_object("video", 100);

        // Assert
        let records = ledger.snapshot();
        assert_eq!(records.len(), 1);
        assert!(!records[0].is_open());
        assert_eq!(records[0].bytes, 0);
    }
}
