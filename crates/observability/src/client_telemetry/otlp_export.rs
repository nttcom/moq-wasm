use serde_json::{Value, json};

use super::report::{ClientLog, ClientReport, PlayerStatValues};
use crate::otlp::{
    Kind::{self, Counter, Gauge},
    Metrics, string_attribute, time_unix_nano,
};

const SERVICE_NAME: &str = "browser";
const SCOPE_NAME: &str = "client-telemetry";
/// Attributes the server sets; a log field of the same name is dropped.
const RESERVED_ATTRIBUTES: [&str; 3] = ["app_id", "client_id", "target"];

/// Name, unit, kind and value of one player data point.
type Measurement = (&'static str, &'static str, Kind, Option<f64>);

fn player_measurements(values: &PlayerStatValues) -> [Measurement; 6] {
    [
        (
            "client.player.viewer_delay",
            "ms",
            Gauge,
            values.viewer_delay_ms,
        ),
        ("client.player.buffer", "ms", Gauge, values.buffer_ms),
        (
            "client.player.received",
            "kbit/s",
            Gauge,
            values.received_kbps,
        ),
        (
            "client.player.audio_breaks",
            "{break}",
            Counter,
            values.audio_breaks,
        ),
        (
            "client.player.audio_concealed",
            "ms",
            Counter,
            values.audio_concealed_ms,
        ),
        ("client.player.shed", "ms", Counter, values.shed_ms),
    ]
}

/// The OTLP/JSON `ExportLogsServiceRequest` body, or `None` when the report has no logs.
pub(super) fn logs_request(report: &ClientReport, app_id: &str) -> Option<Value> {
    if report.logs.is_empty() {
        return None;
    }
    let records: Vec<Value> = report
        .logs
        .iter()
        .map(|log| log_record(log, app_id, &report.client_id))
        .collect();
    Some(json!({
        "resourceLogs": [{
            "resource": resource(report),
            "scopeLogs": [{ "scope": { "name": SCOPE_NAME }, "logRecords": records }],
        }],
    }))
}

/// The OTLP/JSON `ExportMetricsServiceRequest` body, or `None` when the report has no stats.
pub(super) fn metrics_request(report: &ClientReport, app_id: &str) -> Option<Value> {
    if report.stats.is_empty() {
        return None;
    }
    let identity = [
        string_attribute("app_id", app_id),
        string_attribute("client_id", report.client_id.as_str()),
    ];
    let mut metrics = Metrics::default();
    for stats in &report.stats {
        let time_unix_nano = time_unix_nano(stats.time_ms);
        let mut add = |(name, unit, kind, value): Measurement, attributes: &[Value]| {
            if let Some(value) = value.filter(|value| value.is_finite()) {
                let point = json!({
                    "timeUnixNano": time_unix_nano,
                    "asDouble": value,
                    "attributes": attributes,
                });
                metrics.add(name, unit, kind, point);
            }
        };
        for measurement in player_measurements(&stats.values) {
            add(measurement, &identity);
        }
        let gaps = &stats.values.max_arrival_gap_ms;
        for (media_kind, gap) in [("video", gaps.video), ("audio", gaps.audio)] {
            let mut attributes = identity.to_vec();
            attributes.push(string_attribute("media_kind", media_kind));
            add(
                ("client.player.max_arrival_gap", "ms", Gauge, gap),
                &attributes,
            );
        }
    }
    Some(json!({
        "resourceMetrics": [{
            "resource": resource(report),
            "scopeMetrics": [{ "scope": { "name": SCOPE_NAME }, "metrics": metrics.into_otlp() }],
        }],
    }))
}

fn resource(report: &ClientReport) -> Value {
    json!({
        "attributes": [
            string_attribute("service.name", SERVICE_NAME),
            string_attribute("page", report.page.as_str()),
        ],
    })
}

fn log_record(log: &ClientLog, app_id: &str, client_id: &str) -> Value {
    let mut attributes = vec![
        string_attribute("app_id", app_id),
        string_attribute("client_id", client_id),
        string_attribute("target", log.target.as_str()),
    ];
    attributes.extend(
        log.fields
            .iter()
            .filter(|(key, _)| !RESERVED_ATTRIBUTES.contains(&key.as_str()))
            .map(|(key, value)| json!({ "key": key, "value": any_value(value) })),
    );
    let (severity_number, severity_text) = severity(&log.level);
    json!({
        "timeUnixNano": time_unix_nano(log.time_ms),
        "severityNumber": severity_number,
        "severityText": severity_text,
        "body": { "stringValue": log.message },
        "attributes": attributes,
    })
}

/// OTLP's SeverityNumber and text for a `tracing` level name; an unknown name is UNSPECIFIED.
fn severity(level: &str) -> (u8, String) {
    let text = level.to_ascii_uppercase();
    let number = match text.as_str() {
        "TRACE" => 1,
        "DEBUG" => 5,
        "INFO" => 9,
        "WARN" => 13,
        "ERROR" => 17,
        _ => 0,
    };
    (number, text)
}

fn any_value(value: &Value) -> Value {
    match value {
        Value::String(text) => json!({ "stringValue": text }),
        Value::Bool(flag) => json!({ "boolValue": flag }),
        Value::Number(number) => match number.as_i64() {
            Some(int) => json!({ "intValue": int.to_string() }),
            None => json!({ "doubleValue": number.as_f64() }),
        },
        other => json!({ "stringValue": other.to_string() }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{logs_request, metrics_request};
    use crate::client_telemetry::report::ClientReport;

    fn report(body: Value) -> ClientReport {
        ClientReport::parse(body.to_string().as_bytes()).unwrap()
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
    fn a_log_carries_the_server_s_app_id_and_not_the_client_s() {
        // Arrange
        let report = report(json!({
            "clientId": "c1",
            "page": "live-viewer",
            "logs": [{
                "timeMs": 2,
                "level": "warn",
                "target": "wasm::media_broadcast",
                "message": "rendition push failed",
                "fields": { "app_id": "spoofed", "track_name": "video" },
            }],
        }));

        // Act
        let request = logs_request(&report, "anon").unwrap();

        // Assert
        let resource_logs = &request["resourceLogs"][0];
        assert_eq!(
            resource_logs["resource"]["attributes"][0]["value"]["stringValue"],
            "browser"
        );
        let record = &resource_logs["scopeLogs"][0]["logRecords"][0];
        assert_eq!(record["severityNumber"], 13);
        assert_eq!(record["severityText"], "WARN");
        assert_eq!(record["timeUnixNano"], "2000000");
        assert_eq!(record["body"]["stringValue"], "rendition push failed");
        assert_eq!(
            record["attributes"],
            json!([
                { "key": "app_id", "value": { "stringValue": "anon" } },
                { "key": "client_id", "value": { "stringValue": "c1" } },
                { "key": "target", "value": { "stringValue": "wasm::media_broadcast" } },
                { "key": "track_name", "value": { "stringValue": "video" } },
            ])
        );
    }

    #[test]
    fn player_stats_become_points_of_the_client_and_its_app() {
        // Arrange
        let report = report(json!({
            "clientId": "c1",
            "page": "live-viewer",
            "stats": [{
                "timeMs": 1000,
                "values": { "bufferMs": 120.5, "audioBreaks": 3, "maxArrivalGapMs": { "audio": 25 } },
            }],
        }));

        // Act
        let request = metrics_request(&report, "app-1").unwrap();

        // Assert
        let buffer = &metric(&request, "client.player.buffer")["gauge"]["dataPoints"][0];
        assert_eq!(buffer["asDouble"], 120.5);
        assert_eq!(buffer["timeUnixNano"], "1000000000");
        assert_eq!(
            buffer["attributes"],
            json!([
                { "key": "app_id", "value": { "stringValue": "app-1" } },
                { "key": "client_id", "value": { "stringValue": "c1" } },
            ])
        );
        let breaks = &metric(&request, "client.player.audio_breaks")["sum"];
        assert_eq!(breaks["isMonotonic"], true);
        assert_eq!(breaks["dataPoints"][0]["asDouble"], 3.0);
        let gaps = metric(&request, "client.player.max_arrival_gap")["gauge"]["dataPoints"]
            .as_array()
            .unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0]["attributes"][2]["value"]["stringValue"], "audio");
    }

    #[test]
    fn a_report_without_logs_or_stats_exports_nothing() {
        // Arrange
        let report = report(json!({ "clientId": "c1", "page": "p" }));

        // Act / Assert
        assert!(logs_request(&report, "anon").is_none());
        assert!(metrics_request(&report, "anon").is_none());
    }
}
