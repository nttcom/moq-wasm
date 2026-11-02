use serde_json::{Map, Value, json};

use super::report::{ClientLog, ClientReport, ClientSpan, PlayerStatValues};
use crate::otlp::{
    Kind::{self, Counter, Gauge},
    Metrics, string_attribute, time_unix_nano,
};

const SCOPE_NAME: &str = "client-telemetry";
const SPAN_KIND_INTERNAL: u8 = 1;
const STATUS_CODE_OK: u8 = 1;
const STATUS_CODE_ERROR: u8 = 2;
/// Attributes the server sets; a log or span field of the same name is dropped.
const RESERVED_ATTRIBUTES: [&str; 3] = ["app_id", "client_id", "target"];

/// Name, unit, kind and value of one player data point.
type Measurement = (&'static str, &'static str, Kind, Option<f64>);

fn player_measurements(values: &PlayerStatValues) -> [Measurement; 10] {
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
        (
            "client.player.arrival_spread",
            "ms",
            Gauge,
            values.arrival_spread_ms,
        ),
        (
            "client.player.av_sync_offset",
            "ms",
            Gauge,
            values.sync_offset_ms,
        ),
        (
            "client.player.video_dropped",
            "{frame}",
            Counter,
            values.video_dropped_frames,
        ),
        (
            "client.player.video_late",
            "{frame}",
            Counter,
            values.video_late_frames,
        ),
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
    let mut metrics = Metrics::default();
    for stats in &report.stats {
        let mut identity = vec![
            string_attribute("app_id", app_id),
            string_attribute("client_id", report.client_id.as_str()),
        ];
        identity.extend(
            [
                ("track_namespace", &stats.track_namespace),
                ("media_source", &stats.media_source),
            ]
            .into_iter()
            .filter_map(|(key, label)| Some(string_attribute(key, label.as_deref()?))),
        );
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
        let per_media_kind = [
            (
                "client.player.max_arrival_gap",
                &stats.values.max_arrival_gap_ms,
            ),
            (
                "client.player.arrival_delay",
                &stats.values.arrival_delay_ms,
            ),
        ];
        for (name, values) in per_media_kind {
            for (media_kind, value) in [("video", values.video), ("audio", values.audio)] {
                let mut attributes = identity.clone();
                attributes.push(string_attribute("media_kind", media_kind));
                add((name, "ms", Gauge, value), &attributes);
            }
        }
    }
    Some(json!({
        "resourceMetrics": [{
            "resource": resource(report),
            "scopeMetrics": [{ "scope": { "name": SCOPE_NAME }, "metrics": metrics.into_otlp() }],
        }],
    }))
}

/// The OTLP/JSON `ExportTraceServiceRequest` body, or `None` when the report has no spans.
pub(super) fn traces_request(report: &ClientReport, app_id: &str) -> Option<Value> {
    if report.spans.is_empty() {
        return None;
    }
    let spans: Vec<Value> = report
        .spans
        .iter()
        .map(|span| span_record(span, app_id, &report.client_id))
        .collect();
    Some(json!({
        "resourceSpans": [{
            "resource": resource(report),
            "scopeSpans": [{ "scope": { "name": SCOPE_NAME }, "spans": spans }],
        }],
    }))
}

fn resource(report: &ClientReport) -> Value {
    json!({
        "attributes": [
            string_attribute("service.name", report.service.name()),
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
    attributes.extend(field_attributes(&log.fields));
    let (severity_number, severity_text) = severity(&log.level);
    json!({
        "timeUnixNano": time_unix_nano(log.time_ms),
        "severityNumber": severity_number,
        "severityText": severity_text,
        "body": { "stringValue": log.message },
        "attributes": attributes,
    })
}

fn span_record(span: &ClientSpan, app_id: &str, client_id: &str) -> Value {
    let mut attributes = vec![
        string_attribute("app_id", app_id),
        string_attribute("client_id", client_id),
    ];
    attributes.extend(field_attributes(&span.fields));
    let mut record = json!({
        "traceId": span.trace_id,
        "spanId": span.span_id,
        "name": span.name,
        "kind": SPAN_KIND_INTERNAL,
        "startTimeUnixNano": time_unix_nano(span.start_time_ms),
        "endTimeUnixNano": time_unix_nano(span.end_time_ms),
        "attributes": attributes,
    });
    if let Some(parent_span_id) = &span.parent_span_id {
        record["parentSpanId"] = json!(parent_span_id);
    }
    match span.fields.get("outcome").and_then(Value::as_str) {
        Some("ok") => record["status"] = json!({ "code": STATUS_CODE_OK }),
        Some(outcome @ ("rejected" | "failed")) => {
            record["status"] = json!({ "code": STATUS_CODE_ERROR, "message": outcome });
        }
        _ => {}
    }
    record
}

fn field_attributes(fields: &Map<String, Value>) -> impl Iterator<Item = Value> + '_ {
    fields
        .iter()
        .filter(|(key, _)| !RESERVED_ATTRIBUTES.contains(&key.as_str()))
        .map(|(key, value)| json!({ "key": key, "value": any_value(value) }))
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

    use super::{logs_request, metrics_request, traces_request};
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

    fn span_report(fields: Value, parent_span_id: Option<&str>) -> ClientReport {
        report(json!({
            "clientId": "c1",
            "page": "subscribe",
            "service": "moq-cli",
            "spans": [{
                "traceId": "0123456789abcdef0123456789abcdef",
                "spanId": "0123456789abcdef",
                "parentSpanId": parent_span_id,
                "name": "moqt.subscriber.subscribe",
                "startTimeMs": 2,
                "endTimeMs": 5,
                "fields": fields,
            }],
        }))
    }

    fn first_span(request: &Value) -> &Value {
        &request["resourceSpans"][0]["scopeSpans"][0]["spans"][0]
    }

    #[test]
    fn a_span_carries_the_server_s_app_id_and_not_the_client_s() {
        // Arrange
        let report = span_report(json!({ "app_id": "spoofed", "request_id": 4 }), None);

        // Act
        let request = traces_request(&report, "anon").unwrap();

        // Assert
        assert_eq!(
            request["resourceSpans"][0]["resource"]["attributes"][0]["value"]["stringValue"],
            "moq-cli"
        );
        let span = first_span(&request);
        assert_eq!(span["traceId"], "0123456789abcdef0123456789abcdef");
        assert_eq!(span["spanId"], "0123456789abcdef");
        assert_eq!(span["name"], "moqt.subscriber.subscribe");
        assert_eq!(span["kind"], 1);
        assert_eq!(span["startTimeUnixNano"], "2000000");
        assert_eq!(span["endTimeUnixNano"], "5000000");
        assert_eq!(
            span["attributes"],
            json!([
                { "key": "app_id", "value": { "stringValue": "anon" } },
                { "key": "client_id", "value": { "stringValue": "c1" } },
                { "key": "request_id", "value": { "intValue": "4" } },
            ])
        );
    }

    #[test]
    fn a_span_without_a_parent_or_an_outcome_omits_them() {
        // Arrange
        let report = span_report(json!({}), None);

        // Act
        let request = traces_request(&report, "anon").unwrap();

        // Assert
        let span = first_span(&request).as_object().unwrap();
        assert!(!span.contains_key("parentSpanId"));
        assert!(!span.contains_key("status"));
    }

    #[test]
    fn a_span_s_outcome_becomes_its_status() {
        // Arrange
        let ok = span_report(json!({ "outcome": "ok" }), Some("fedcba9876543210"));
        let rejected = span_report(json!({ "outcome": "rejected" }), None);

        // Act
        let ok = traces_request(&ok, "anon").unwrap();
        let rejected = traces_request(&rejected, "anon").unwrap();

        // Assert
        assert_eq!(first_span(&ok)["parentSpanId"], "fedcba9876543210");
        assert_eq!(first_span(&ok)["status"], json!({ "code": 1 }));
        assert_eq!(
            first_span(&rejected)["status"],
            json!({ "code": 2, "message": "rejected" })
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
                "values": {
                    "bufferMs": 120.5,
                    "audioBreaks": 3,
                    "syncOffsetMs": -6.5,
                    "videoDroppedFrames": 4,
                    "maxArrivalGapMs": { "audio": 25 },
                    "arrivalDelayMs": { "audio": 8, "video": 2254 },
                },
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
        let sync_offset = &metric(&request, "client.player.av_sync_offset")["gauge"];
        assert_eq!(sync_offset["dataPoints"][0]["asDouble"], -6.5);
        let dropped = &metric(&request, "client.player.video_dropped")["sum"];
        assert_eq!(dropped["dataPoints"][0]["asDouble"], 4.0);
        let gaps = metric(&request, "client.player.max_arrival_gap")["gauge"]["dataPoints"]
            .as_array()
            .unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0]["attributes"][2]["value"]["stringValue"], "audio");
        let delays = metric(&request, "client.player.arrival_delay")["gauge"]["dataPoints"]
            .as_array()
            .unwrap();
        assert_eq!(delays.len(), 2);
        assert_eq!(delays[0]["attributes"][2]["value"]["stringValue"], "video");
        assert_eq!(delays[0]["asDouble"], 2254.0);
        assert_eq!(delays[1]["attributes"][2]["value"]["stringValue"], "audio");
        assert_eq!(delays[1]["asDouble"], 8.0);
    }

    #[test]
    fn stats_of_one_track_among_several_carry_its_namespace_and_source() {
        // Arrange
        let report = report(json!({
            "clientId": "c1",
            "page": "meeting",
            "stats": [{
                "timeMs": 1000,
                "trackNamespace": "anon/meeting/room/alice",
                "mediaSource": "screenshare",
                "values": { "bufferMs": 80 },
            }],
        }));

        // Act
        let request = metrics_request(&report, "anon").unwrap();

        // Assert
        let buffer = &metric(&request, "client.player.buffer")["gauge"]["dataPoints"][0];
        assert_eq!(
            buffer["attributes"],
            json!([
                { "key": "app_id", "value": { "stringValue": "anon" } },
                { "key": "client_id", "value": { "stringValue": "c1" } },
                { "key": "track_namespace", "value": { "stringValue": "anon/meeting/room/alice" } },
                { "key": "media_source", "value": { "stringValue": "screenshare" } },
            ])
        );
    }

    #[test]
    fn a_report_without_logs_stats_or_spans_exports_nothing() {
        // Arrange
        let report = report(json!({ "clientId": "c1", "page": "p" }));

        // Act / Assert
        assert!(logs_request(&report, "anon").is_none());
        assert!(metrics_request(&report, "anon").is_none());
        assert!(traces_request(&report, "anon").is_none());
    }
}
