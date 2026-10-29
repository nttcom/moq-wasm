use std::collections::BTreeMap;

use Kind::{Counter, Gauge};
use relay_stats::{ProcessStats, RelaySnapshot, SessionStats};
use serde_json::{Value, json};

#[derive(Clone, Copy)]
enum Kind {
    Gauge,
    /// A total since the session started.
    Counter,
}

/// Name, unit, kind and value of one data point.
type Measurement = (&'static str, &'static str, Kind, u64);

fn process_measurements(process: &ProcessStats) -> Vec<Measurement> {
    let mut measurements = vec![
        ("relay.cache.tracks", "{track}", Gauge, process.cache_tracks),
        (
            "relay.cache.objects",
            "{object}",
            Gauge,
            process.cache_objects,
        ),
        (
            "relay.cache.payload",
            "By",
            Gauge,
            process.cache_payload_bytes,
        ),
        (
            "relay.disk_cache.payload",
            "By",
            Gauge,
            process.disk_cache_bytes,
        ),
        (
            "relay.client_sessions",
            "{session}",
            Gauge,
            process.client_sessions,
        ),
    ];
    if let Some(rss_bytes) = process.rss_bytes {
        measurements.push(("relay.process.rss", "By", Gauge, rss_bytes));
    }
    measurements
}

fn session_measurements(s: &SessionStats) -> [Measurement; 14] {
    [
        ("relay.session.rtt", "us", Gauge, s.rtt_us),
        ("relay.session.cwnd", "By", Gauge, s.cwnd),
        ("relay.session.mtu", "By", Gauge, u64::from(s.current_mtu)),
        ("relay.session.sent", "By", Counter, s.sent_bytes),
        ("relay.session.received", "By", Counter, s.received_bytes),
        (
            "relay.session.sent_packets",
            "{packet}",
            Counter,
            s.sent_packets,
        ),
        (
            "relay.session.lost_packets",
            "{packet}",
            Counter,
            s.lost_packets,
        ),
        (
            "relay.session.congestion_events",
            "{event}",
            Counter,
            s.congestion_events,
        ),
        (
            "relay.session.sent_stream_data_blocked",
            "{frame}",
            Counter,
            s.sent_stream_data_blocked,
        ),
        (
            "relay.session.sent_data_blocked",
            "{frame}",
            Counter,
            s.sent_data_blocked,
        ),
        (
            "relay.session.received_stream_data_blocked",
            "{frame}",
            Counter,
            s.received_stream_data_blocked,
        ),
        (
            "relay.session.received_data_blocked",
            "{frame}",
            Counter,
            s.received_data_blocked,
        ),
        (
            "relay.session.received_stop_sending",
            "{frame}",
            Counter,
            s.received_stop_sending,
        ),
        (
            "relay.session.received_reset_stream",
            "{frame}",
            Counter,
            s.received_reset_stream,
        ),
    ]
}

fn string_attribute(key: &str, value: impl Into<Value>) -> Value {
    json!({ "key": key, "value": { "stringValue": value.into() } })
}

/// The OTLP/JSON `ExportMetricsServiceRequest` body.
pub(crate) fn export_request(snapshots: &[RelaySnapshot]) -> Value {
    let resource_metrics: Vec<Value> = snapshots.iter().map(resource_metrics).collect();
    json!({ "resourceMetrics": resource_metrics })
}

fn resource_metrics(snapshot: &RelaySnapshot) -> Value {
    let time_unix_nano = (u128::from(snapshot.timestamp_ms) * 1_000_000).to_string();
    let mut metrics: BTreeMap<&'static str, (&'static str, Kind, Vec<Value>)> = BTreeMap::new();
    let mut add = |(name, unit, kind, value): Measurement, attributes: &[Value]| {
        let points = &mut metrics
            .entry(name)
            .or_insert_with(|| (unit, kind, Vec::new()))
            .2;
        points.push(json!({
            "timeUnixNano": time_unix_nano,
            "asInt": value.to_string(),
            "attributes": attributes,
        }));
    };
    for measurement in process_measurements(&snapshot.process) {
        add(measurement, &[]);
    }
    for session in &snapshot.sessions {
        let attributes = [
            string_attribute("session_id", session.session_id.to_string()),
            string_attribute("session_peer", json!(session.peer)),
            string_attribute("app_id", session.app_id.as_str()),
        ];
        for measurement in session_measurements(session) {
            add(measurement, &attributes);
        }
    }
    let metrics: Vec<Value> = metrics
        .into_iter()
        .map(|(name, (unit, kind, data_points))| match kind {
            Kind::Gauge => json!({
                "name": name,
                "unit": unit,
                "gauge": { "dataPoints": data_points },
            }),
            Kind::Counter => json!({
                "name": name,
                "unit": unit,
                "sum": {
                    // OTLP AggregationTemporality 2 is CUMULATIVE.
                    "aggregationTemporality": 2,
                    "isMonotonic": true,
                    "dataPoints": data_points,
                },
            }),
        })
        .collect();
    json!({
        "resource": {
            "attributes": [
                string_attribute("service.name", "relay"),
                string_attribute("relay.id", snapshot.relay_id.as_str()),
            ],
        },
        "scopeMetrics": [{
            "scope": { "name": "observability" },
            "metrics": metrics,
        }],
    })
}

#[cfg(test)]
mod tests {
    use relay_stats::{SessionPeer, SessionStats};
    use serde_json::Value;

    use super::export_request;
    use crate::test_support;

    fn session() -> SessionStats {
        SessionStats {
            session_id: 42,
            peer: SessionPeer::Client,
            app_id: "app-1".to_string(),
            remote_address: None,
            local_ip: None,
            dialed_relay_id: None,
            rtt_us: 1500,
            current_mtu: 1200,
            sent_bytes: 9000,
            sent_packets: 10,
            lost_packets: 1,
            cwnd: 12000,
            congestion_events: 0,
            sent_stream_data_blocked: 0,
            sent_data_blocked: 0,
            received_stop_sending: 0,
            received_bytes: 300,
            received_stream_data_blocked: 0,
            received_data_blocked: 0,
            received_reset_stream: 0,
        }
    }

    fn metric<'a>(request: &'a Value, name: &str) -> &'a Value {
        request["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|metric| metric["name"] == name)
            .unwrap_or_else(|| panic!("{name} is missing"))
    }

    #[test]
    fn a_relay_is_a_relay_resource_keyed_by_its_id() {
        // Arrange
        let snapshot = test_support::snapshot(1_000);

        // Act
        let request = export_request(&[snapshot]);

        // Assert
        assert_eq!(
            request["resourceMetrics"][0]["resource"]["attributes"],
            serde_json::json!([
                { "key": "service.name", "value": { "stringValue": "relay" } },
                { "key": "relay.id", "value": { "stringValue": "relay-a" } },
            ])
        );
    }

    #[test]
    fn a_session_s_rtt_is_a_gauge_point_named_by_the_session() {
        // Arrange
        let mut snapshot = test_support::snapshot(1_000);
        snapshot.sessions.push(session());

        // Act
        let request = export_request(&[snapshot]);

        // Assert
        let point = &metric(&request, "relay.session.rtt")["gauge"]["dataPoints"][0];
        assert_eq!(point["asInt"], "1500");
        assert_eq!(point["timeUnixNano"], "1000000000");
        assert_eq!(
            point["attributes"],
            serde_json::json!([
                { "key": "session_id", "value": { "stringValue": "42" } },
                { "key": "session_peer", "value": { "stringValue": "client" } },
                { "key": "app_id", "value": { "stringValue": "app-1" } },
            ])
        );
    }

    #[test]
    fn a_session_s_sent_bytes_are_a_cumulative_monotonic_sum() {
        // Arrange
        let mut snapshot = test_support::snapshot(1_000);
        snapshot.sessions.push(session());

        // Act
        let request = export_request(&[snapshot]);

        // Assert
        let sum = &metric(&request, "relay.session.sent")["sum"];
        assert_eq!(sum["aggregationTemporality"], 2);
        assert_eq!(sum["isMonotonic"], true);
        assert_eq!(sum["dataPoints"][0]["asInt"], "9000");
    }
}
