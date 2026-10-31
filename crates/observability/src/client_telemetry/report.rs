use anyhow::ensure;
use serde::Deserialize;
use serde_json::{Map, Value};

/// The most logs, and the most stats, one report may carry.
const MAX_RECORDS: usize = 100;
/// Longest `clientId` and `page`: both become attributes of every point.
const MAX_LABEL_CHARS: usize = 128;

/// The JSON body a browser posts. Unknown keys are ignored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClientReport {
    pub(super) token: Option<String>,
    pub(super) client_id: String,
    pub(super) page: String,
    #[serde(default)]
    pub(super) logs: Vec<ClientLog>,
    #[serde(default)]
    pub(super) stats: Vec<PlayerStats>,
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

/// One sample of `LivePlayer.stats()`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PlayerStats {
    pub(super) time_ms: u64,
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
    #[serde(default)]
    pub(super) max_arrival_gap_ms: ArrivalGaps,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct ArrivalGaps {
    pub(super) video: Option<f64>,
    pub(super) audio: Option<f64>,
}

impl ClientReport {
    pub(super) fn parse(body: &[u8]) -> anyhow::Result<Self> {
        let report: Self = serde_json::from_slice(body)?;
        ensure!(
            report.logs.len() <= MAX_RECORDS && report.stats.len() <= MAX_RECORDS,
            "a report carries at most {MAX_RECORDS} logs and {MAX_RECORDS} stats"
        );
        for (name, label) in [("clientId", &report.client_id), ("page", &report.page)] {
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

    use super::{ClientReport, MAX_RECORDS};

    #[test]
    fn a_report_reads_the_stats_it_knows_and_ignores_the_rest() {
        // Arrange
        let body = json!({
            "clientId": "c1",
            "page": "live-viewer",
            "stats": [{ "timeMs": 1, "values": { "bufferMs": 120.5, "videoDrops": "0/0", "maxArrivalGapMs": { "video": 40 } } }],
        });

        // Act
        let report = ClientReport::parse(body.to_string().as_bytes()).unwrap();

        // Assert
        let values = &report.stats[0].values;
        assert_eq!(values.buffer_ms, Some(120.5));
        assert_eq!(values.viewer_delay_ms, None);
        assert_eq!(values.max_arrival_gap_ms.video, Some(40.0));
        assert!(report.logs.is_empty());
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
    fn a_report_without_a_client_id_is_rejected() {
        // Arrange
        let body = json!({ "clientId": "", "page": "p" });

        // Act / Assert
        assert!(ClientReport::parse(body.to_string().as_bytes()).is_err());
    }
}
