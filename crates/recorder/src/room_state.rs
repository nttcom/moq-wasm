use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde_json::{Value, json};

use crate::recording::Layout;

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

/// A room recording as `state/<id>.json` holds it. The API writes it once
/// as `Starting`; from then on only the recording job writes it, stamping
/// `updated_at_ms` (Unix ms) each time.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RoomRecord {
    pub(crate) id: String,
    pub(crate) namespace_prefix: String,
    pub(crate) layout: Layout,
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
    fn a_record_reads_back_what_it_wrote() {
        // Arrange
        let record = RoomRecord {
            id: "1700000000000-a1b2c3".to_string(),
            namespace_prefix: "anon/research".to_string(),
            layout: Layout::PerTrack,
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
    }
}
