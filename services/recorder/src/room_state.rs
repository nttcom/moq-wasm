use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::recording::Layout;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RecordingFormat {
    #[default]
    Mp4,
    Moq,
}

/// How a room is recorded: the `options` `bots` passes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecordingOptions {
    pub(crate) layout: Layout,
    pub(crate) composite: bool,
    pub(crate) format: RecordingFormat,
}

impl RecordingOptions {
    /// `layout` (`combined` by default), `composite` (`false`) and `format`
    /// (`mp4`) of a JSON object; the error names the field that is invalid.
    pub(crate) fn from_json(body: &Value) -> Result<Self, &'static str> {
        let layout = match body.get("layout").and_then(Value::as_str) {
            None => Layout::Combined,
            Some(name) => {
                Layout::from_str(name, false).map_err(|_| "layout must be combined or per-track")?
            }
        };
        let composite = match body.get("composite") {
            None => false,
            Some(Value::Bool(composite)) => *composite,
            Some(_) => return Err("composite must be a boolean"),
        };
        let format = match body.get("format") {
            None => RecordingFormat::Mp4,
            Some(Value::String(name)) => {
                RecordingFormat::from_str(name, false).map_err(|_| "format must be mp4 or moq")?
            }
            Some(_) => return Err("format must be mp4 or moq"),
        };
        Ok(Self {
            layout,
            composite,
            format,
        })
    }
}

/// The `state` and `error` fields a record and a composite state are written
/// with.
#[derive(Serialize, Deserialize)]
struct StateFields {
    state: String,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(into = "StateFields", try_from = "StateFields")]
pub(crate) enum RoomState {
    /// Requested; the recording job has not connected yet.
    Starting,
    Recording,
    Stopping,
    Finished,
    Failed(String),
}

impl RoomState {
    pub(crate) fn is_active(&self) -> bool {
        matches!(self, Self::Starting | Self::Recording | Self::Stopping)
    }
}

impl From<RoomState> for StateFields {
    fn from(state: RoomState) -> Self {
        let (state, error) = match state {
            RoomState::Starting => ("starting", None),
            RoomState::Recording => ("recording", None),
            RoomState::Stopping => ("stopping", None),
            RoomState::Finished => ("finished", None),
            RoomState::Failed(error) => ("failed", Some(error)),
        };
        Self {
            state: state.to_string(),
            error,
        }
    }
}

impl TryFrom<StateFields> for RoomState {
    type Error = String;

    fn try_from(fields: StateFields) -> Result<Self, String> {
        Ok(match fields.state.as_str() {
            "starting" => Self::Starting,
            "recording" => Self::Recording,
            "stopping" => Self::Stopping,
            "finished" => Self::Finished,
            "failed" => Self::Failed(fields.error.unwrap_or_default()),
            other => return Err(format!("unknown recording state {other}")),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct MemberStatus {
    pub(crate) namespace: String,
    pub(crate) started_at_ms: u64,
    #[serde(default)]
    pub(crate) finished: bool,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(into = "StateFields", try_from = "StateFields")]
pub(crate) enum CompositeState {
    /// While the room is recorded and until the composite starts.
    Waiting,
    Composing,
    Done,
    Skipped(String),
    Failed(String),
}

impl From<CompositeState> for StateFields {
    fn from(state: CompositeState) -> Self {
        let (state, error) = match state {
            CompositeState::Waiting => ("waiting", None),
            CompositeState::Composing => ("composing", None),
            CompositeState::Done => ("done", None),
            CompositeState::Skipped(reason) => ("skipped", Some(reason)),
            CompositeState::Failed(error) => ("failed", Some(error)),
        };
        Self {
            state: state.to_string(),
            error,
        }
    }
}

impl TryFrom<StateFields> for CompositeState {
    type Error = String;

    fn try_from(fields: StateFields) -> Result<Self, String> {
        let error = fields.error.unwrap_or_default();
        Ok(match fields.state.as_str() {
            "waiting" => Self::Waiting,
            "composing" => Self::Composing,
            "done" => Self::Done,
            "skipped" => Self::Skipped(error),
            "failed" => Self::Failed(error),
            other => return Err(format!("unknown composite state {other}")),
        })
    }
}

/// A room recording as `state/<id>.json` holds it. The recording job writes
/// it, first as `Starting`, stamping `updated_at_ms` (Unix ms) each time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RoomRecord {
    pub(crate) id: String,
    pub(crate) namespace_prefix: String,
    pub(crate) layout: Layout,
    /// `mp4` in a record written before MoQ recordings.
    #[serde(default)]
    pub(crate) format: RecordingFormat,
    /// Whether the room is composited once the recording ends.
    #[serde(default)]
    pub(crate) composite: bool,
    #[serde(flatten)]
    pub(crate) state: RoomState,
    #[serde(default)]
    pub(crate) members: Vec<MemberStatus>,
    #[serde(default)]
    pub(crate) updated_at_ms: u64,
}

#[cfg(test)]
mod tests {
    use serde_json::{from_value, json, to_value};

    use super::*;

    #[test]
    fn recording_options_default_to_a_combined_mp4_without_composite() {
        // Act
        let defaults = RecordingOptions::from_json(&json!({})).unwrap();
        let invalid = RecordingOptions::from_json(&json!({ "format": "webm" }));

        // Assert
        assert_eq!(
            defaults,
            RecordingOptions {
                layout: Layout::Combined,
                composite: false,
                format: RecordingFormat::Mp4,
            }
        );
        assert_eq!(invalid, Err("format must be mp4 or moq"));
    }

    #[test]
    fn a_record_reads_back_what_it_wrote() {
        // Arrange
        let record = RoomRecord {
            id: "1700000000000-a1b2c3".to_string(),
            namespace_prefix: "anon/research".to_string(),
            layout: Layout::PerTrack,
            format: RecordingFormat::Mp4,
            composite: true,
            state: RoomState::Failed("relay gone".to_string()),
            members: vec![MemberStatus {
                namespace: "anon/research/ucchy".to_string(),
                started_at_ms: 1_700_000_000_100,
                finished: true,
                error: None,
            }],
            updated_at_ms: 1_700_000_030_000,
        };

        // Act
        let read: RoomRecord = from_value(to_value(&record).unwrap()).unwrap();

        // Assert
        assert_eq!(read, record);
        let mut legacy = to_value(&record).unwrap();
        legacy.as_object_mut().unwrap().remove("format");
        assert_eq!(
            from_value::<RoomRecord>(legacy.clone()).unwrap().format,
            RecordingFormat::Mp4
        );
        legacy["format"] = json!(17);
        assert!(from_value::<RoomRecord>(legacy).is_err());
    }

    #[test]
    fn a_record_and_a_composite_state_keep_their_json_shape() {
        // Arrange
        let record = RoomRecord {
            id: "1700000000000-a1b2c3".to_string(),
            namespace_prefix: "anon/research".to_string(),
            layout: Layout::Combined,
            format: RecordingFormat::Moq,
            composite: false,
            state: RoomState::Recording,
            members: vec![MemberStatus {
                namespace: "anon/research/ucchy".to_string(),
                started_at_ms: 1_700_000_000_100,
                finished: false,
                error: None,
            }],
            updated_at_ms: 1_700_000_030_000,
        };

        // Act
        let record_json = to_value(&record).unwrap().to_string();
        let waiting = to_value(CompositeState::Waiting).unwrap().to_string();
        let skipped = to_value(CompositeState::Skipped("no media".to_string()))
            .unwrap()
            .to_string();

        // Assert
        assert_eq!(
            record_json,
            concat!(
                r#"{"composite":false,"error":null,"format":"moq","id":"1700000000000-a1b2c3","#,
                r#""layout":"combined","members":[{"error":null,"finished":false,"#,
                r#""namespace":"anon/research/ucchy","started_at_ms":1700000000100}],"#,
                r#""namespace_prefix":"anon/research","state":"recording","updated_at_ms":1700000030000}"#,
            )
        );
        assert_eq!(waiting, r#"{"error":null,"state":"waiting"}"#);
        assert_eq!(skipped, r#"{"error":"no media","state":"skipped"}"#);
    }

    #[test]
    fn reads_a_record_with_only_its_required_fields() {
        // Arrange
        let record = json!({
            "id": "1700000000000-a1b2c3",
            "namespace_prefix": "anon/research",
            "layout": "per-track",
            "state": "failed",
            "members": [{ "namespace": "anon/research/ucchy", "started_at_ms": 1 }],
        });
        let composite = json!({ "state": "failed" });

        // Act
        let record: RoomRecord = from_value(record).unwrap();
        let composite: CompositeState = from_value(composite).unwrap();

        // Assert
        assert_eq!(
            record,
            RoomRecord {
                id: "1700000000000-a1b2c3".to_string(),
                namespace_prefix: "anon/research".to_string(),
                layout: Layout::PerTrack,
                format: RecordingFormat::Mp4,
                composite: false,
                state: RoomState::Failed(String::new()),
                members: vec![MemberStatus {
                    namespace: "anon/research/ucchy".to_string(),
                    started_at_ms: 1,
                    finished: false,
                    error: None,
                }],
                updated_at_ms: 0,
            }
        );
        assert_eq!(composite, CompositeState::Failed(String::new()));
        assert!(from_value::<CompositeState>(json!({ "state": "lost" })).is_err());
    }
}
