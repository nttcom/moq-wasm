use relay_stats::RelaySnapshot;
use serde_json::Value;

/// JavaScript numbers hold 53 bits, so the browser receives 64-bit ids as strings.
const ID_FIELDS: &[&str] = &[
    "session_id",
    "publisher_session_id",
    "subscriber_session_id",
    "request_id",
];
const SECTIONS: &[&str] = &["sessions", "tracks", "subscriptions"];

pub fn snapshots_for_browser(snapshots: &[RelaySnapshot]) -> Value {
    Value::Array(snapshots.iter().map(snapshot_for_browser).collect())
}

fn snapshot_for_browser(snapshot: &RelaySnapshot) -> Value {
    let mut value =
        serde_json::to_value(snapshot).expect("a snapshot holds only serializable plain data");
    for section in SECTIONS {
        let Some(Value::Array(entries)) = value.get_mut(*section) else {
            continue;
        };
        for entry in entries {
            for field in ID_FIELDS {
                if let Some(id) = entry.get_mut(*field)
                    && let Some(number) = id.as_u64()
                {
                    *id = Value::String(number.to_string());
                }
            }
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use relay_stats::{ProcessStats, RelaySnapshot, SubscriptionStats};
    use serde_json::json;

    use super::snapshots_for_browser;

    #[test]
    fn ids_beyond_53_bits_reach_the_browser_unrounded() {
        // Arrange
        let snapshot = RelaySnapshot {
            relay_id: "relay-a".to_string(),
            timestamp_ms: 1,
            process: ProcessStats::default(),
            sessions: vec![],
            tracks: vec![],
            subscriptions: vec![SubscriptionStats {
                namespace: "app".to_string(),
                name: "video".to_string(),
                publisher_session_id: 1_791_278_627_397_623_670,
                subscriber_session_id: 1_791_278_649_845_683_583,
                request_id: 2,
                forward: true,
                objects_sent: 3,
                bytes_sent: 4,
                streams_opened: 5,
                streams_reset: 0,
                lag_behind_newest_received_us: 6,
            }],
        };

        // Act
        let value = snapshots_for_browser(&[snapshot]);

        // Assert
        let subscription = &value[0]["subscriptions"][0];
        assert_eq!(
            subscription["publisher_session_id"],
            json!("1791278627397623670")
        );
        assert_eq!(
            subscription["subscriber_session_id"],
            json!("1791278649845683583")
        );
        assert_eq!(subscription["request_id"], json!("2"));
        assert_eq!(subscription["bytes_sent"], json!(4));
    }
}
