use std::net::{IpAddr, SocketAddr};

use relay_stats::{RelaySnapshot, SessionPeer};
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

fn snapshot_for_browser(snapshot: &RelaySnapshot) -> Value {
    let mut snapshot = snapshot.clone();
    for session in &mut snapshot.sessions {
        if session.peer == SessionPeer::Client {
            session.remote_address = session.remote_address.as_deref().map(masked_client_address);
        }
    }
    let mut value =
        serde_json::to_value(&snapshot).expect("a snapshot holds only serializable plain data");
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

    use super::{masked_client_address, snapshots_for_browser};

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
                bytes_sent: 4,
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
