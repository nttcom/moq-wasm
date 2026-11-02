use anyhow::ensure;
use serde::Deserialize;
use serde_json::{Map, Value};

/// The most logs, the most stats, and the most spans one report may carry.
const MAX_RECORDS: usize = 100;
/// Longest label a report may carry: each becomes a metric attribute.
const MAX_LABEL_CHARS: usize = 128;

/// The JSON body a client posts. Unknown keys are ignored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClientReport {
    pub(super) token: Option<String>,
    #[serde(default)]
    pub(super) service: ClientService,
    pub(super) client_id: String,
    pub(super) page: String,
    #[serde(default)]
    pub(super) logs: Vec<ClientLog>,
    #[serde(default)]
    pub(super) stats: Vec<PlayerStats>,
    #[serde(default)]
    pub(super) spans: Vec<ClientSpan>,
}

/// The kind of client, which becomes `service.name`. A fixed set, so a client
/// cannot pose as a server's service.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ClientService {
    #[default]
    Browser,
    MoqCli,
    Python,
}

impl ClientService {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::MoqCli => "moq-cli",
            Self::Python => "python",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClientLog {
    pub(super) time_ms: u64,
    pub(super) level: String,
    pub(super) target: String,
    pub(super) message: String,
    #[serde(default)]
    pub(super) fields: Map<String, Value>,
}

/// A closed `tracing` span. Ids are lowercase hex, as OTLP/JSON carries them.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClientSpan {
    pub(super) trace_id: String,
    pub(super) span_id: String,
    pub(super) parent_span_id: Option<String>,
    pub(super) name: String,
    pub(super) start_time_ms: u64,
    pub(super) end_time_ms: u64,
    #[serde(default)]
    pub(super) fields: Map<String, Value>,
}

/// One sample of a player's `stats()`. `trackNamespace` and `mediaSource` are
/// absent when the page plays a single track.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PlayerStats {
    pub(super) time_ms: u64,
    pub(super) track_namespace: Option<String>,
    pub(super) media_source: Option<String>,
    pub(super) values: PlayerStatValues,
}

/// The fields of `LivePlayerStats` that become metrics; a missing one has no point.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PlayerStatValues {
    pub(super) viewer_delay_ms: Option<f64>,
    pub(super) buffer_ms: Option<f64>,
    pub(super) received_kbps: Option<f64>,
    pub(super) audio_breaks: Option<f64>,
    pub(super) audio_concealed_ms: Option<f64>,
    pub(super) shed_ms: Option<f64>,
    pub(super) arrival_spread_ms: Option<f64>,
    pub(super) sync_offset_ms: Option<f64>,
    pub(super) video_dropped_frames: Option<f64>,
    pub(super) video_late_frames: Option<f64>,
    #[serde(default)]
    pub(super) max_arrival_gap_ms: PerMediaKind,
    #[serde(default)]
    pub(super) arrival_delay_ms: PerMediaKind,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct PerMediaKind {
    pub(super) video: Option<f64>,
    pub(super) audio: Option<f64>,
}

impl ClientReport {
    pub(super) fn parse(body: &[u8]) -> anyhow::Result<Self> {
        let report: Self = serde_json::from_slice(body)?;
        ensure!(
            report.logs.len() <= MAX_RECORDS
                && report.stats.len() <= MAX_RECORDS
                && report.spans.len() <= MAX_RECORDS,
            "a report carries at most {MAX_RECORDS} logs, stats and spans each"
        );
        // A malformed id makes the collector reject the whole batch.
        for span in &report.spans {
            let ids = [(&span.trace_id, 32), (&span.span_id, 16)]
                .into_iter()
                .chain(span.parent_span_id.iter().map(|id| (id, 16)));
            for (id, len) in ids {
                ensure!(
                    id.len() == len && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
                    "span id {id:?} is not {len} hex digits"
                );
            }
        }
        let stats_labels = report
            .stats
            .iter()
            .flat_map(|stats| {
                [
                    ("trackNamespace", &stats.track_namespace),
                    ("mediaSource", &stats.media_source),
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
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ClientReport, ClientService, MAX_RECORDS};

    #[test]
    fn a_report_reads_the_stats_it_knows_and_ignores_the_rest() {
        // Arrange
        let body = json!({
            "clientId": "c1",
            "page": "live-viewer",
            "stats": [{ "timeMs": 1, "values": { "bufferMs": 120.5, "videoDrops": "0/0", "maxArrivalGapMs": { "video": 40 }, "arrivalDelayMs": { "audio": 8, "video": 2254 } } }],
        });

        // Act
        let report = ClientReport::parse(body.to_string().as_bytes()).unwrap();

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
        let cli = ClientReport::parse(cli.to_string().as_bytes()).unwrap();
        let browser = ClientReport::parse(browser.to_string().as_bytes()).unwrap();

        // Assert
        assert_eq!(cli.service, ClientService::MoqCli);
        assert_eq!(browser.service, ClientService::Browser);
    }

    #[test]
    fn a_report_of_an_unknown_service_is_rejected() {
        // Arrange
        let body = json!({ "clientId": "c1", "page": "p", "service": "relay" });

        // Act / Assert
        assert!(ClientReport::parse(body.to_string().as_bytes()).is_err());
    }

    #[test]
    fn a_report_with_too_many_logs_is_rejected() {
        // Arrange
        let log = json!({ "timeMs": 1, "level": "warn", "target": "moqt", "message": "m" });
        let body = json!({ "clientId": "c1", "page": "p", "logs": vec![log; MAX_RECORDS + 1] });

        // Act / Assert
        assert!(ClientReport::parse(body.to_string().as_bytes()).is_err());
    }

    #[test]
    fn a_span_with_a_malformed_trace_id_is_rejected() {
        // Arrange
        let span = json!({ "traceId": "xyz", "spanId": "0123456789abcdef", "name": "s", "startTimeMs": 1, "endTimeMs": 2 });
        let body = json!({ "clientId": "c1", "page": "p", "spans": [span] });

        // Act / Assert
        assert!(ClientReport::parse(body.to_string().as_bytes()).is_err());
    }

    #[test]
    fn a_track_namespace_over_the_label_limit_is_rejected() {
        // Arrange
        let stats = json!({ "timeMs": 1, "trackNamespace": "n".repeat(129), "values": {} });
        let body = json!({ "clientId": "c1", "page": "p", "stats": [stats] });

        // Act / Assert
        assert!(ClientReport::parse(body.to_string().as_bytes()).is_err());
    }

    #[test]
    fn a_report_without_a_client_id_is_rejected() {
        // Arrange
        let body = json!({ "clientId": "", "page": "p" });

        // Act / Assert
        assert!(ClientReport::parse(body.to_string().as_bytes()).is_err());
    }
}
