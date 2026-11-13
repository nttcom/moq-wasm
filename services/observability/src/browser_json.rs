use std::{
    collections::HashSet,
    net::{IpAddr, SocketAddr},
};

use serde_json::Value;
use telemetry::relay_stats::{RelaySnapshot, SessionPeer};

/// JavaScript numbers hold 53 bits, so the browser receives 64-bit ids as strings.
const ID_FIELDS: &[&str] = &[
    "session_id",
    "publisher_session_id",
    "subscriber_session_id",
    "request_id",
];
const SECTIONS: &[&str] = &["sessions", "tracks", "subscriptions"];
/// What a relay session keeps: enough to follow a track to the relay it came
/// from, none of the counters every app's traffic adds to.
const RELAY_SESSION_FIELDS: &[&str] = &[
    "session_id",
    "peer",
    "remote_address",
    "local_ip",
    "dialed_relay_id",
];

/// Each relay's snapshot as `app_id` may see it: its client sessions, the
/// tracks and subscriptions under its namespace root, and the relay sessions
/// those name, without their counters. The relay's process stats carry every
/// app's traffic, so they are left out; the relay's id stays.
pub fn snapshots_for_browser(snapshots: &[RelaySnapshot], app_id: &str) -> Value {
    Value::Array(
        snapshots
            .iter()
            .map(|snapshot| snapshot_for_browser(snapshot, app_id))
            .collect(),
    )
}

/// Whether `namespace` is the app's root or lies under it, element by element.
pub(crate) fn in_app(namespace: &str, app_id: &str) -> bool {
    namespace
        .strip_prefix(app_id)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn masked_client_address(address: &str) -> String {
    let Ok(socket) = address.parse::<SocketAddr>() else {
        return "masked".to_string();
    };
    let ip = match socket.ip() {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            format!("{a}.{b}.{c}.x:{}", socket.port())
        }
        IpAddr::V6(v6) => {
            let [a, b, c, ..] = v6.segments();
            format!("[{a:x}:{b:x}:{c:x}::x]:{}", socket.port())
        }
    }
}

fn snapshot_for_browser(snapshot: &RelaySnapshot, app_id: &str) -> Value {
    let mut snapshot = snapshot.clone();
    snapshot
        .tracks
        .retain(|track| in_app(&track.namespace, app_id));
    snapshot
        .subscriptions
        .retain(|subscription| in_app(&subscription.namespace, app_id));
    let named: HashSet<u64> = snapshot
        .tracks
        .iter()
        .map(|track| track.publisher_session_id)
        .chain(snapshot.subscriptions.iter().flat_map(|subscription| {
            [
                subscription.publisher_session_id,
                subscription.subscriber_session_id,
            ]
        }))
        .collect();
    snapshot.sessions.retain(|session| match session.peer {
        SessionPeer::Client => session.app_id == app_id,
        SessionPeer::Relay => named.contains(&session.session_id),
        SessionPeer::StatsPublisher | SessionPeer::StatsSubscriber => false,
    });
    for session in &mut snapshot.sessions {
        if session.peer == SessionPeer::Client {
            session.remote_address = session.remote_address.as_deref().map(masked_client_address);
        }
    }
    let mut value =
        serde_json::to_value(&snapshot).expect("a snapshot holds only serializable plain data");
    if let Value::Object(fields) = &mut value {
        fields.remove("process");
    }
    if let Some(Value::Array(sessions)) = value.get_mut("sessions") {
        for session in sessions {
            if session["peer"] == "relay"
                && let Value::Object(fields) = session
            {
                fields.retain(|field, _| RELAY_SESSION_FIELDS.contains(&field.as_str()));
            }
        }
    }
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
    use serde_json::json;
    use telemetry::relay_stats::{
        RelaySnapshot, SessionPeer, SessionStats, SubscriptionStats, TrackStats,
    };

    use super::{in_app, masked_client_address, snapshots_for_browser};
    use crate::test_support::snapshot;

    #[test]
    fn a_client_address_keeps_its_network_and_port_but_not_its_host() {
        // Act / Assert
        assert_eq!(
            masked_client_address("203.0.113.5:50123"),
            "203.0.113.x:50123"
        );
        assert_eq!(
            masked_client_address("[::ffff:192.168.65.1]:40015"),
            "192.168.65.x:40015"
        );
        assert_eq!(
            masked_client_address("[2001:db8:1:2::5]:443"),
            "[2001:db8:1::x]:443"
        );
        assert_eq!(masked_client_address("not an address"), "masked");
    }

    #[test]
    fn ids_beyond_53_bits_reach_the_browser_unrounded() {
        // Arrange
        let snapshot = RelaySnapshot {
            subscriptions: vec![SubscriptionStats {
                namespace: "app/room".to_string(),
                name: "video".to_string(),
                publisher_session_id: 1_791_278_627_397_623_670,
                subscriber_session_id: 1_791_278_649_845_683_583,
                request_id: 2,
                bytes_sent: 4,
                streams_reset: 0,
                datagrams_dropped: 0,
                lag_behind_newest_received_us: 6,
            }],
            ..snapshot(1)
        };

        // Act
        let value = snapshots_for_browser(&[snapshot], "app");

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

    fn session(session_id: u64, peer: SessionPeer, app_id: &str) -> SessionStats {
        SessionStats {
            session_id,
            peer,
            app_id: app_id.to_string(),
            remote_address: Some("203.0.113.5:50123".to_string()),
            local_ip: None,
            dialed_relay_id: None,
            rtt_us: 0,
            current_mtu: 1200,
            sent_bytes: 0,
            sent_packets: 0,
            lost_packets: 0,
            cwnd: 0,
            congestion_events: 0,
            sent_stream_data_blocked: 0,
            sent_data_blocked: 0,
            received_stop_sending: 0,
            received_bytes: 0,
            received_stream_data_blocked: 0,
            received_data_blocked: 0,
            received_reset_stream: 0,
        }
    }

    fn track(namespace: &str) -> TrackStats {
        TrackStats {
            namespace: namespace.to_string(),
            name: "video".to_string(),
            publisher_session_id: 1,
            bytes_received: 0,
            max_arrival_gap_since_last_snapshot_us: 0,
        }
    }

    #[test]
    fn an_app_sees_its_own_sessions_and_namespaces_but_not_the_relays_stats() {
        // Arrange
        let snapshot = RelaySnapshot {
            sessions: vec![
                session(1, SessionPeer::Client, "app"),
                session(2, SessionPeer::Client, "other"),
                session(3, SessionPeer::Relay, "relay"),
            ],
            tracks: vec![
                track("app/room"),
                track("other/room"),
                track("application/room"),
            ],
            ..snapshot(1)
        };

        // Act
        let value = snapshots_for_browser(&[snapshot], "app");

        // Assert: the relay itself is still named, but not its process stats or relay links
        let relay = &value[0];
        assert_eq!(relay["relay_id"], json!("relay-a"));
        assert!(relay.get("process").is_none());
        assert_eq!(relay["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(relay["sessions"][0]["session_id"], json!("1"));
        assert_eq!(
            relay["sessions"][0]["remote_address"],
            json!("203.0.113.x:50123")
        );
        assert_eq!(relay["tracks"].as_array().unwrap().len(), 1);
        assert_eq!(relay["tracks"][0]["namespace"], json!("app/room"));
    }

    #[test]
    fn a_namespace_is_in_an_app_on_element_boundaries() {
        // Act / Assert
        assert!(in_app("app", "app"));
        assert!(in_app("app/room/alice", "app"));
        assert!(!in_app("application/room", "app"));
        assert!(!in_app("other/app", "app"));
    }
}
