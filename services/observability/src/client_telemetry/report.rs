use anyhow::ensure;
use telemetry::client_report::{ClientReport, MAX_RECORDS};

/// Longest label a report may carry: each becomes a metric attribute.
const MAX_LABEL_CHARS: usize = 128;

pub(super) fn parse_report(body: &[u8]) -> anyhow::Result<ClientReport> {
    let report: ClientReport = serde_json::from_slice(body)?;
    ensure!(
        report.logs.len() <= MAX_RECORDS
            && report.stats.len() <= MAX_RECORDS
            && report.publisher_stats.len() <= MAX_RECORDS
            && report.spans.len() <= MAX_RECORDS,
        "a report carries at most {MAX_RECORDS} logs, stats, publisher stats and spans each"
    );
    // A malformed id makes the collector reject the whole batch.
    let span_ids = report.spans.iter().flat_map(|span| {
        [
            (Some(&span.trace_id), 32),
            (Some(&span.span_id), 16),
            (span.parent_span_id.as_ref(), 16),
        ]
    });
    let log_ids = report
        .logs
        .iter()
        .flat_map(|log| [(log.trace_id.as_ref(), 32), (log.span_id.as_ref(), 16)]);
    for (id, len) in span_ids
        .chain(log_ids)
        .filter_map(|(id, len)| Some((id?, len)))
    {
        ensure!(
            id.len() == len && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "id {id:?} is not {len} hex digits"
        );
    }
    let player_labels = report
        .stats
        .iter()
        .map(|stats| (&stats.track_namespace, &stats.media_source));
    let publisher_labels = report
        .publisher_stats
        .iter()
        .map(|stats| (&stats.track_namespace, &stats.media_source));
    let stats_labels = player_labels
        .chain(publisher_labels)
        .flat_map(|(track_namespace, media_source)| {
            [
                ("trackNamespace", track_namespace),
                ("mediaSource", media_source),
            ]
        })
        .filter_map(|(name, label)| Some((name, label.as_ref()?)));
    let required = [("clientId", &report.client_id), ("page", &report.page)];
    for (name, label) in required.into_iter().chain(stats_labels) {
        ensure!(
            !label.is_empty() && label.chars().count() <= MAX_LABEL_CHARS,
            "{name} is empty or longer than {MAX_LABEL_CHARS} characters"
        );
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use telemetry::client_report::{ClientService, MAX_RECORDS};

    use super::parse_report;

    #[test]
    fn a_report_reads_the_stats_it_knows_and_ignores_the_rest() {
        // Arrange
        let body = json!({
            "clientId": "c1",
            "page": "live-viewer",
            "stats": [{ "timeMs": 1, "values": { "bufferMs": 120.5, "videoDrops": "0/0", "maxArrivalGapMs": { "video": 40 }, "arrivalDelayMs": { "audio": 8, "video": 2254 } } }],
        });

        // Act
        let report = parse_report(body.to_string().as_bytes()).unwrap();

        // Assert
        let values = &report.stats[0].values;
        assert_eq!(values.buffer_ms, Some(120.5));
        assert_eq!(values.viewer_delay_ms, None);
        assert_eq!(values.max_arrival_gap_ms.video, Some(40.0));
        assert_eq!(values.arrival_delay_ms.audio, Some(8.0));
        assert_eq!(values.arrival_delay_ms.video, Some(2254.0));
        assert!(report.logs.is_empty());
    }

    #[test]
    fn a_report_names_its_client_service_and_defaults_to_the_browser() {
        // Arrange
        let cli = json!({ "clientId": "c1", "page": "subscribe", "service": "moq-cli" });
        let browser = json!({ "clientId": "c1", "page": "live-viewer" });

        // Act
        let cli = parse_report(cli.to_string().as_bytes()).unwrap();
        let browser = parse_report(browser.to_string().as_bytes()).unwrap();

        // Assert
        assert_eq!(cli.service, ClientService::MoqCli);
        assert_eq!(browser.service, ClientService::Browser);
    }

    #[test]
    fn a_report_of_an_unknown_service_is_rejected() {
        // Arrange
        let body = json!({ "clientId": "c1", "page": "p", "service": "relay" });

        // Act / Assert
        assert!(parse_report(body.to_string().as_bytes()).is_err());
    }

    #[test]
    fn a_report_with_too_many_logs_is_rejected() {
        // Arrange
        let log = json!({ "timeMs": 1, "level": "warn", "target": "moqt", "message": "m" });
        let body = json!({ "clientId": "c1", "page": "p", "logs": vec![log; MAX_RECORDS + 1] });

        // Act / Assert
        assert!(parse_report(body.to_string().as_bytes()).is_err());
    }

    #[test]
    fn a_span_with_a_malformed_trace_id_is_rejected() {
        // Arrange
        let span = json!({ "traceId": "xyz", "spanId": "0123456789abcdef", "name": "s", "startTimeMs": 1, "endTimeMs": 2 });
        let body = json!({ "clientId": "c1", "page": "p", "spans": [span] });

        // Act / Assert
        assert!(parse_report(body.to_string().as_bytes()).is_err());
    }

    #[test]
    fn a_log_with_a_malformed_span_id_is_rejected() {
        // Arrange
        let log = json!({ "timeMs": 1, "level": "info", "target": "moqt", "message": "m", "traceId": "0123456789abcdef0123456789abcdef", "spanId": "xyz" });
        let body = json!({ "clientId": "c1", "page": "p", "logs": [log] });

        // Act / Assert
        assert!(parse_report(body.to_string().as_bytes()).is_err());
    }

    #[test]
    fn a_track_namespace_over_the_label_limit_is_rejected() {
        // Arrange
        let stats = json!({ "timeMs": 1, "trackNamespace": "n".repeat(129), "values": {} });
        let body = json!({ "clientId": "c1", "page": "p", "stats": [stats] });

        // Act / Assert
        assert!(parse_report(body.to_string().as_bytes()).is_err());
    }

    #[test]
    fn a_publisher_s_media_source_over_the_label_limit_is_rejected() {
        // Arrange
        let stats = json!({ "timeMs": 1, "mediaSource": "s".repeat(129), "audio": {} });
        let body = json!({ "clientId": "c1", "page": "p", "publisherStats": [stats] });

        // Act / Assert
        assert!(parse_report(body.to_string().as_bytes()).is_err());
    }

    #[test]
    fn a_report_without_a_client_id_is_rejected() {
        // Arrange
        let body = json!({ "clientId": "", "page": "p" });

        // Act / Assert
        assert!(parse_report(body.to_string().as_bytes()).is_err());
    }
}
