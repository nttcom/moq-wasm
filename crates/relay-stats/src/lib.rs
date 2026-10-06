use anyhow::Context;
use serde::{Deserialize, Serialize};

pub const NAMESPACE_ROOT: &str = "observability";
pub const TRACK_NAME: &str = "network_stats";

pub fn track_namespace(relay_id: &str) -> String {
    format!("{NAMESPACE_ROOT}/{relay_id}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelaySnapshot {
    pub relay_id: String,
    pub timestamp_ms: u64,
    pub process: ProcessStats,
    pub sessions: Vec<SessionStats>,
    pub tracks: Vec<TrackStats>,
    pub subscriptions: Vec<SubscriptionStats>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessStats {
    pub rss_bytes: Option<u64>,
    pub cache_tracks: u64,
    pub cache_objects: u64,
    pub cache_payload_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPeer {
    Client,
    Relay,
    StatsPublisher,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStats {
    pub session_id: u64,
    pub peer: SessionPeer,
    pub app_id: String,
    pub remote_address: Option<String>,
    pub local_ip: Option<String>,
    pub dialed_relay_id: Option<String>,
    pub rtt_us: u64,
    pub current_mtu: u16,
    pub sent_bytes: u64,
    pub sent_packets: u64,
    pub lost_packets: u64,
    pub lost_bytes: u64,
    pub cwnd: u64,
    pub congestion_events: u64,
    pub sent_stream_data_blocked: u64,
    pub sent_data_blocked: u64,
    pub received_stop_sending: u64,
    pub received_bytes: u64,
    pub received_stream_data_blocked: u64,
    pub received_data_blocked: u64,
    pub received_reset_stream: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackStats {
    pub namespace: String,
    pub name: String,
    pub publisher_session_id: u64,
    pub objects_received: u64,
    pub bytes_received: u64,
    pub subgroups_aborted: u64,
    pub max_arrival_gap_since_last_snapshot_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionStats {
    pub namespace: String,
    pub name: String,
    pub publisher_session_id: u64,
    pub subscriber_session_id: u64,
    pub request_id: u64,
    pub forward: bool,
    pub objects_sent: u64,
    pub bytes_sent: u64,
    pub streams_opened: u64,
    pub streams_reset: u64,
    pub lag_behind_newest_received_us: u64,
}

impl RelaySnapshot {
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("a snapshot holds only serializable plain data")
    }

    pub fn from_json(bytes: &[u8]) -> anyhow::Result<Self> {
        serde_json::from_slice(bytes).context("payload is not a relay snapshot")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> RelaySnapshot {
        RelaySnapshot {
            relay_id: "relay-a".to_string(),
            timestamp_ms: 1_791_262_800_000,
            process: ProcessStats {
                rss_bytes: Some(123),
                cache_tracks: 1,
                cache_objects: 2,
                cache_payload_bytes: 3,
            },
            sessions: vec![SessionStats {
                session_id: 7,
                peer: SessionPeer::Client,
                app_id: "app".to_string(),
                remote_address: Some("192.0.2.1:50000".to_string()),
                local_ip: Some("192.0.2.2".to_string()),
                dialed_relay_id: Some("relay-b".to_string()),
                rtt_us: 12_000,
                current_mtu: 1452,
                sent_bytes: 1,
                sent_packets: 2,
                lost_packets: 3,
                lost_bytes: 4,
                cwnd: 5,
                congestion_events: 6,
                sent_stream_data_blocked: 7,
                sent_data_blocked: 8,
                received_stop_sending: 9,
                received_bytes: 10,
                received_stream_data_blocked: 11,
                received_data_blocked: 12,
                received_reset_stream: 13,
            }],
            tracks: vec![TrackStats {
                namespace: "app/live".to_string(),
                name: "video".to_string(),
                publisher_session_id: 7,
                objects_received: 1,
                bytes_received: 2,
                subgroups_aborted: 3,
                max_arrival_gap_since_last_snapshot_us: 4,
            }],
            subscriptions: vec![SubscriptionStats {
                namespace: "app/live".to_string(),
                name: "video".to_string(),
                publisher_session_id: 7,
                subscriber_session_id: 9,
                request_id: 2,
                forward: true,
                objects_sent: 1,
                bytes_sent: 2,
                streams_opened: 3,
                streams_reset: 4,
                lag_behind_newest_received_us: 5,
            }],
        }
    }

    #[test]
    fn json_round_trip_keeps_every_field() {
        // Arrange
        let snapshot = snapshot();

        // Act
        let decoded = RelaySnapshot::from_json(&snapshot.to_json()).unwrap();

        // Assert
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn peers_are_written_in_snake_case() {
        // Act
        let json = String::from_utf8(snapshot().to_json()).unwrap();

        // Assert
        assert!(json.contains(r#""peer":"client""#));
    }

    #[test]
    fn track_namespace_is_rooted_at_observability() {
        // Act / Assert
        assert_eq!(track_namespace("relay-a"), "observability/relay-a");
    }
}
