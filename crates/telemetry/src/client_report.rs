//! The body clients post to the observability server's `/api/client-telemetry`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The observability server rejects a report carrying more logs, stats or
/// spans than this, each kind counted separately.
pub const MAX_RECORDS: usize = 100;

/// Unknown keys are ignored.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientReport {
    pub token: Option<String>,
    #[serde(default)]
    pub service: ClientService,
    pub client_id: String,
    pub page: String,
    #[serde(default)]
    pub logs: Vec<ClientLog>,
    #[serde(default)]
    pub stats: Vec<PlayerStats>,
    #[serde(default)]
    pub spans: Vec<ClientSpan>,
}

/// The kind of client, which becomes `service.name`. A fixed set, so a client
/// cannot pose as a server's service.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientService {
    #[default]
    Browser,
    MoqCli,
    Python,
}

impl ClientService {
    pub fn name(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::MoqCli => "moq-cli",
            Self::Python => "python",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientLog {
    pub time_ms: u64,
    pub level: String,
    pub target: String,
    pub message: String,
    #[serde(default)]
    pub fields: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
}

impl ClientLog {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a log holds only plain JSON data")
    }
}

/// A closed `tracing` span. Ids are lowercase hex, as OTLP/JSON carries them.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientSpan {
    pub trace_id: String,
    pub span_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    pub name: String,
    pub start_time_ms: u64,
    pub end_time_ms: u64,
    #[serde(default)]
    pub fields: Map<String, Value>,
}

impl ClientSpan {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a span holds only plain JSON data")
    }
}

/// One sample of a player's `stats()`. `trackNamespace` and `mediaSource` are
/// absent when the page plays a single track.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStats {
    pub time_ms: u64,
    pub track_namespace: Option<String>,
    pub media_source: Option<String>,
    pub values: PlayerStatValues,
}

/// The fields of `LivePlayerStats` that become metrics; a missing one has no point.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStatValues {
    pub viewer_delay_ms: Option<f64>,
    pub buffer_ms: Option<f64>,
    /// The video track's received bitrate.
    pub received_kbps: Option<f64>,
    pub received_audio_kbps: Option<f64>,
    pub audio_breaks: Option<f64>,
    pub audio_concealed_ms: Option<f64>,
    pub shed_ms: Option<f64>,
    pub arrival_spread_ms: Option<f64>,
    pub sync_offset_ms: Option<f64>,
    pub video_dropped_frames: Option<f64>,
    pub video_late_frames: Option<f64>,
    #[serde(default)]
    pub max_arrival_gap_ms: PerMediaKind,
    #[serde(default)]
    pub arrival_delay_ms: PerMediaKind,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PerMediaKind {
    pub video: Option<f64>,
    pub audio: Option<f64>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ClientLog, ClientSpan};

    #[test]
    fn a_log_and_a_span_without_ids_leave_them_out() {
        // Arrange
        let log = ClientLog {
            time_ms: 1,
            level: "info".into(),
            target: "moqt".into(),
            message: "m".into(),
            fields: Default::default(),
            trace_id: None,
            span_id: None,
        };
        let span = ClientSpan {
            trace_id: "t".into(),
            span_id: "s".into(),
            parent_span_id: None,
            name: "n".into(),
            start_time_ms: 1,
            end_time_ms: 2,
            fields: Default::default(),
        };

        // Act
        let log = serde_json::to_value(&log).unwrap();
        let span = serde_json::to_value(&span).unwrap();

        // Assert
        assert_eq!(
            log,
            json!({ "timeMs": 1, "level": "info", "target": "moqt", "message": "m", "fields": {} })
        );
        assert_eq!(
            span,
            json!({ "traceId": "t", "spanId": "s", "name": "n", "startTimeMs": 1, "endTimeMs": 2, "fields": {} })
        );
    }
}
