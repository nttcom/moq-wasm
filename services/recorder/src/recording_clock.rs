use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use mediapack::loc::{TIMESCALE_ID, TIMESTAMP_ID};
use moqt::object::{KeyValuePair, Object, VariantType};
use serde_json::{Value, json};

const MICROS_PER_SECOND: u64 = 1_000_000;
pub(crate) const MICROS_PER_MILLI: u64 = 1_000;

/// The clock a room recording's archives are stored on: time since the
/// recording started. Each wall-clock LOC Timestamp becomes media time with a Timescale
/// of 1,000,000 (draft-ietf-moq-loc-04 §2.3.1.2), and each wall-clock record
/// (`t`) of an event timeline becomes a media time record (`m`,
/// draft-ietf-moq-msf-01 §8.1), so every participant shares one time axis and
/// the replay's media timelines need no wallclock (§7.1.1).
#[derive(Debug, Clone, Copy)]
pub(crate) struct RecordingClock {
    started_at_ms: u64,
}

impl RecordingClock {
    pub(crate) fn starting_now() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self::started_at(now.as_millis() as u64)
    }

    pub(crate) fn started_at(started_at_ms: u64) -> Self {
        Self { started_at_ms }
    }

    pub(crate) fn started_at_ms(&self) -> u64 {
        self.started_at_ms
    }

    /// `object` with its wall-clock Timestamp as media time; `None` for one
    /// captured before the recording started. An object without a wall-clock
    /// Timestamp is kept as it is.
    pub(crate) fn media_object(&self, mut object: Object) -> Option<Object> {
        let pairs = &mut object.extension_headers.key_value_pairs;
        if pairs.iter().any(|pair| pair.key == TIMESCALE_ID) {
            return Some(object);
        }
        let Some(timestamp) = pairs.iter_mut().find(|pair| pair.key == TIMESTAMP_ID) else {
            return Some(object);
        };
        let VariantType::Even(capture_us) = timestamp.value else {
            return Some(object);
        };
        let media_us = capture_us.checked_sub(self.started_at_ms * MICROS_PER_MILLI)?;
        timestamp.value = VariantType::Even(media_us);
        pairs.push(KeyValuePair {
            key: TIMESCALE_ID,
            value: VariantType::Even(MICROS_PER_SECOND),
        });
        Some(object)
    }

    /// An event timeline document with each `t` record as an `m` record, and
    /// those from before the recording started left out. A payload that is
    /// not a JSON array of records is kept as it is.
    pub(crate) fn event_timeline(&self, payload: &Bytes) -> Bytes {
        let Ok(records) = serde_json::from_slice::<Vec<Value>>(payload) else {
            return payload.clone();
        };
        let records: Vec<Value> = records
            .into_iter()
            .filter_map(|mut record| {
                let Some(wallclock_ms) = record.get("t").and_then(Value::as_u64) else {
                    return Some(record);
                };
                let media_ms = wallclock_ms.checked_sub(self.started_at_ms)?;
                let fields = record.as_object_mut()?;
                fields.remove("t");
                fields.insert("m".to_string(), json!(media_ms));
                Some(record)
            })
            .collect();
        serde_json::to_vec(&records).map_or_else(|_| payload.clone(), Bytes::from)
    }
}

#[cfg(test)]
mod tests {
    use moqt::object::{ExtensionHeaders, ForwardingPreference, Location, ObjectStatus};

    use super::*;

    fn object(timestamp_us: u64) -> Object {
        Object {
            location: Location {
                group_id: 1,
                object_id: 0,
            },
            forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
            publisher_priority: 0,
            status: ObjectStatus::Normal,
            extension_headers: ExtensionHeaders::new(vec![KeyValuePair {
                key: TIMESTAMP_ID,
                value: VariantType::Even(timestamp_us),
            }]),
            payload: Bytes::from_static(b"frame"),
            ends_group: false,
        }
    }

    #[test]
    fn stores_a_capture_timestamp_as_media_time_since_the_start() {
        // Arrange
        let clock = RecordingClock::started_at(1_700_000_000_000);

        // Act
        let stored = clock.media_object(object(1_700_000_002_500_000)).unwrap();

        // Assert
        let object = mediapack::loc::LocObject {
            extensions: mediapack::loc::from_extension_headers(&stored.extension_headers),
            payload: stored.payload,
        };
        assert_eq!(object.capture_timestamp(), None);
        assert_eq!(object.timestamp().unwrap().micros(), 2_500_000);
    }

    #[test]
    fn leaves_out_an_object_captured_before_the_start() {
        // Arrange
        let clock = RecordingClock::started_at(1_700_000_000_000);

        // Act / Assert
        assert_eq!(clock.media_object(object(1_699_999_999_999_000)), None);
    }

    #[test]
    fn turns_wallclock_records_into_media_time_records() {
        // Arrange
        let clock = RecordingClock::started_at(1_000);
        let payload = Bytes::from(
            r#"[{"t":900,"data":"before"},{"t":1500,"data":"hello"},{"L":[1,0],"data":"kept"}]"#,
        );

        // Act
        let stored: Value = serde_json::from_slice(&clock.event_timeline(&payload)).unwrap();

        // Assert
        assert_eq!(
            stored,
            json!([{"m": 500, "data": "hello"}, {"L": [1, 0], "data": "kept"}])
        );
    }

    #[test]
    fn keeps_a_payload_that_is_no_event_timeline() {
        // Arrange
        let clock = RecordingClock::started_at(1_000);
        let payload = Bytes::from_static(b"not json");

        // Act / Assert
        assert_eq!(clock.event_timeline(&payload), payload);
    }
}
