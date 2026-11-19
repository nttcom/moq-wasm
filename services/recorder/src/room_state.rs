use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde_json::{Value, json};

use crate::recording::Layout;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum RecordingFormat {
    Mp4,
    Moq,
}

impl RecordingFormat {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Moq => "moq",
        }
    }
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

#[derive(Debug, Clone, PartialEq)]
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

    pub(crate) fn parts(&self) -> (&'static str, Option<&str>) {
        match self {
            Self::Starting => ("starting", None),
            Self::Recording => ("recording", None),
            Self::Stopping => ("stopping", None),
            Self::Finished => ("finished", None),
            Self::Failed(error) => ("failed", Some(error)),
        }
    }

    fn from_parts(state: &str, error: Option<&str>) -> Result<Self> {
        Ok(match state {
            "starting" => Self::Starting,
            "recording" => Self::Recording,
            "stopping" => Self::Stopping,
            "finished" => Self::Finished,
            "failed" => Self::Failed(error.unwrap_or_default().to_string()),
            other => bail!("unknown recording state {other}"),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MemberStatus {
    pub(crate) namespace: String,
    pub(crate) started_at_ms: u64,
    pub(crate) finished: bool,
    pub(crate) error: Option<String>,
}

impl MemberStatus {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "namespace": self.namespace,
            "started_at_ms": self.started_at_ms,
            "finished": self.finished,
            "error": self.error,
        })
    }

    fn from_json(value: &Value) -> Result<Self> {
        Ok(Self {
            namespace: string(value, "namespace")?,
            started_at_ms: value["started_at_ms"]
                .as_u64()
                .context("member without started_at_ms")?,
            finished: value["finished"].as_bool().unwrap_or_default(),
            error: value["error"].as_str().map(str::to_string),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CompositeState {
    /// While the room is recorded and until the composite starts.
    Waiting,
    Composing,
    Done,
    Skipped(String),
    Failed(String),
}

impl CompositeState {
    pub(crate) fn to_json(&self) -> Value {
        let (state, error) = match self {
            Self::Waiting => ("waiting", None),
            Self::Composing => ("composing", None),
            Self::Done => ("done", None),
            Self::Skipped(reason) => ("skipped", Some(reason.as_str())),
            Self::Failed(error) => ("failed", Some(error.as_str())),
        };
        json!({ "state": state, "error": error })
    }

    pub(crate) fn from_json(value: &Value) -> Result<Self> {
        let error = value["error"].as_str().unwrap_or_default().to_string();
        Ok(match value["state"].as_str() {
            Some("waiting") => Self::Waiting,
            Some("composing") => Self::Composing,
            Some("done") => Self::Done,
            Some("skipped") => Self::Skipped(error),
            Some("failed") => Self::Failed(error),
            other => bail!("unknown composite state {other:?}"),
        })
    }
}

/// A room recording as `state/<id>.json` holds it. The recording job writes
/// it, first as `Starting`, stamping `updated_at_ms` (Unix ms) each time.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RoomRecord {
    pub(crate) id: String,
    pub(crate) namespace_prefix: String,
    pub(crate) layout: Layout,
    pub(crate) format: RecordingFormat,
    /// Whether the room is composited once the recording ends.
    pub(crate) composite: bool,
    pub(crate) state: RoomState,
    pub(crate) members: Vec<MemberStatus>,
    pub(crate) updated_at_ms: u64,
}

impl RoomRecord {
    pub(crate) fn to_json(&self) -> Value {
        let (state, error) = self.state.parts();
        json!({
            "id": self.id,
            "namespace_prefix": self.namespace_prefix,
            "layout": self.layout.name(),
            "format": self.format.name(),
            "composite": self.composite,
            "state": state,
            "error": error,
            "members": self.members.iter().map(MemberStatus::to_json).collect::<Vec<_>>(),
            "updated_at_ms": self.updated_at_ms,
        })
    }

    pub(crate) fn from_json(value: &Value) -> Result<Self> {
        let layout = string(value, "layout")?;
        Ok(Self {
            id: string(value, "id")?,
            namespace_prefix: string(value, "namespace_prefix")?,
            layout: Layout::from_str(&layout, false)
                .map_err(|error| anyhow::anyhow!("layout {layout}: {error}"))?,
            format: match value.get("format") {
                None => RecordingFormat::Mp4,
                Some(Value::String(format)) => RecordingFormat::from_str(format, false)
                    .map_err(|error| anyhow::anyhow!("recording format {format}: {error}"))?,
                Some(_) => bail!("recording format must be a string"),
            },
            composite: value["composite"].as_bool().unwrap_or_default(),
            state: RoomState::from_parts(&string(value, "state")?, value["error"].as_str())?,
            members: value["members"]
                .as_array()
                .map(|members| members.iter().map(MemberStatus::from_json).collect())
                .transpose()?
                .unwrap_or_default(),
            updated_at_ms: value["updated_at_ms"].as_u64().unwrap_or_default(),
        })
    }
}

fn string(value: &Value, key: &str) -> Result<String> {
    value[key]
        .as_str()
        .map(str::to_string)
        .with_context(|| format!("no {key}"))
}

#[cfg(test)]
mod tests {
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
        let read = RoomRecord::from_json(&record.to_json()).unwrap();

        // Assert
        assert_eq!(read, record);
        let mut legacy = record.to_json();
        legacy.as_object_mut().unwrap().remove("format");
        assert_eq!(
            RoomRecord::from_json(&legacy).unwrap().format,
            RecordingFormat::Mp4
        );
        legacy["format"] = json!(17);
        assert!(RoomRecord::from_json(&legacy).is_err());
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
        let record_json = record.to_json().to_string();
        let waiting = CompositeState::Waiting.to_json().to_string();
        let skipped = CompositeState::Skipped("no media".to_string())
            .to_json()
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
        // Arrange: a member without finished or error, and a failed composite without error
        let record = json!({
            "id": "1700000000000-a1b2c3",
            "namespace_prefix": "anon/research",
            "layout": "per-track",
            "state": "failed",
            "members": [{ "namespace": "anon/research/ucchy", "started_at_ms": 1 }],
        });
        let composite = json!({ "state": "failed" });

        // Act
        let record = RoomRecord::from_json(&record).unwrap();
        let composite = CompositeState::from_json(&composite).unwrap();

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
        assert!(CompositeState::from_json(&json!({ "state": "lost" })).is_err());
    }
}
