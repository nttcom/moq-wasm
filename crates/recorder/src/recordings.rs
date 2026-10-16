use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use object_store::{ObjectStore, path::Path};
use serde_json::{Value, json};

use crate::{
    composite::CompositeQueue,
    recorder::BroadcastOptions,
    recording::Layout,
    room_recording::{
        CompositeState, MemberStatus, RecordingLimits, RoomConfig, RoomRecording, RoomState,
    },
};

/// What every recording the server starts shares.
pub(crate) struct ServeConfig {
    pub(crate) relay: String,
    pub(crate) verify_certificate: bool,
    /// The first element every recorded namespace prefix must have.
    pub(crate) namespace_root: String,
    pub(crate) max_recordings: usize,
    pub(crate) output_dir: PathBuf,
    pub(crate) upload: Option<(Arc<dyn ObjectStore>, Path)>,
    pub(crate) max_video_height: u32,
    pub(crate) segment_length: Option<Duration>,
    pub(crate) limits: RecordingLimits,
    /// `None` refuses compositing. With an upload store, the most bytes of
    /// participants' files kept on disk for compositing at once.
    pub(crate) composite_budget_bytes: Option<u64>,
}

#[derive(Debug)]
pub(crate) enum StartRejection {
    /// The prefix is outside `namespace_root` or names no room.
    OutsideRoot,
    AlreadyRecording,
    TooManyRecordings,
    CompositeDisabled,
}

/// The recordings the server started, finished ones included, so that
/// `GET /recordings/{id}` keeps answering after a recording ended.
pub(crate) struct Recordings {
    config: ServeConfig,
    composite: Option<CompositeQueue>,
    entries: Mutex<Vec<Entry>>,
}

struct Entry {
    id: String,
    namespace_prefix: String,
    layout: Layout,
    recording: RoomRecording,
}

impl Recordings {
    pub(crate) fn new(config: ServeConfig) -> Self {
        Self {
            composite: config.composite_budget_bytes.map(CompositeQueue::new),
            config,
            entries: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn start(
        &self,
        namespace_prefix: &str,
        layout: Layout,
        composite: bool,
    ) -> Result<Value, StartRejection> {
        if !within_root(namespace_prefix, &self.config.namespace_root) {
            return Err(StartRejection::OutsideRoot);
        }
        if composite && self.composite.is_none() {
            return Err(StartRejection::CompositeDisabled);
        }
        let mut entries = self.lock();
        let active: Vec<&Entry> = entries.iter().filter(|entry| entry.is_active()).collect();
        if active
            .iter()
            .any(|entry| entry.namespace_prefix == namespace_prefix)
        {
            return Err(StartRejection::AlreadyRecording);
        }
        if active.len() >= self.config.max_recordings {
            return Err(StartRejection::TooManyRecordings);
        }
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let id = format!("{now_ms}-{}", entries.len() + 1);
        let recording = RoomRecording::run(RoomConfig {
            relay: self.config.relay.clone(),
            verify_certificate: self.config.verify_certificate,
            namespace_prefix: namespace_prefix.to_string(),
            output_dir: self.config.output_dir.join(&id),
            upload: self.config.upload.as_ref().map(|(store, prefix)| {
                let mut prefix = prefix.clone();
                prefix.extend([id.as_str()]);
                (Arc::clone(store), prefix)
            }),
            options: BroadcastOptions {
                video_track: None,
                max_video_height: self.config.max_video_height,
                audio_track: None,
                layout,
                from_cache: false,
                segment_length: self.config.segment_length,
                upload: None,
            },
            limits: self.config.limits,
            composite: self.composite.clone().filter(|_| composite),
        });
        tracing::info!(%id, namespace_prefix, ?layout, composite, "recording requested");
        entries.push(Entry {
            id,
            namespace_prefix: namespace_prefix.to_string(),
            layout,
            recording,
        });
        Ok(entries.last().map(Entry::to_json).unwrap_or_default())
    }

    pub(crate) fn stop(&self, id: &str) -> Option<Value> {
        let entries = self.lock();
        let entry = entries.iter().find(|entry| entry.id == id)?;
        entry.recording.stop();
        Some(entry.to_json())
    }

    pub(crate) fn get(&self, id: &str) -> Option<Value> {
        self.lock()
            .iter()
            .find(|entry| entry.id == id)
            .map(Entry::to_json)
    }

    pub(crate) fn list(&self, namespace_prefix: Option<&str>) -> Value {
        let recordings: Vec<Value> = self
            .lock()
            .iter()
            .filter(|entry| namespace_prefix.is_none_or(|prefix| entry.namespace_prefix == prefix))
            .map(Entry::to_json)
            .collect();
        json!({ "recordings": recordings })
    }

    /// Stops every recording and waits until each has closed and uploaded
    /// its files. Composites not made yet are given up.
    pub(crate) async fn stop_all(&self) {
        let mut recordings: Vec<RoomRecording> =
            self.lock().drain(..).map(|entry| entry.recording).collect();
        for recording in &recordings {
            recording.stop();
            recording.cancel_composite();
        }
        for recording in &mut recordings {
            recording.finished().await;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Entry {
    fn is_active(&self) -> bool {
        matches!(
            self.recording.status().state,
            RoomState::Recording | RoomState::Stopping
        )
    }

    fn to_json(&self) -> Value {
        let status = self.recording.status();
        let (state, error) = match &status.state {
            RoomState::Recording => ("recording", None),
            RoomState::Stopping => ("stopping", None),
            RoomState::Finished => ("finished", None),
            RoomState::Failed(error) => ("failed", Some(error.as_str())),
        };
        json!({
            "id": self.id,
            "namespace_prefix": self.namespace_prefix,
            "layout": self.layout.name(),
            "state": state,
            "error": error,
            "members": status.members.iter().map(member_json).collect::<Vec<_>>(),
            "composite": status.composite.as_ref().map(composite_json),
        })
    }
}

fn composite_json(composite: &CompositeState) -> Value {
    let (state, error) = match composite {
        CompositeState::Waiting => ("waiting", None),
        CompositeState::Composing => ("composing", None),
        CompositeState::Done => ("done", None),
        CompositeState::Skipped(reason) => ("skipped", Some(reason.as_str())),
        CompositeState::Failed(error) => ("failed", Some(error.as_str())),
    };
    json!({ "state": state, "error": error })
}

fn member_json(member: &MemberStatus) -> Value {
    json!({
        "namespace": member.namespace,
        "started_at_ms": member.started_at_ms,
        "finished": member.finished,
        "error": member.error,
    })
}

/// `<root>/<room>[/...]` with no empty element.
fn within_root(namespace_prefix: &str, root: &str) -> bool {
    let parts: Vec<&str> = namespace_prefix.split('/').collect();
    parts.len() >= 2 && parts[0] == root && parts.iter().all(|part| !part.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_a_room_under_the_root() {
        // Act / Assert
        assert!(within_root("anon/research", "anon"));
        assert!(within_root("anon/research/ucchy", "anon"));
        assert!(!within_root("anon", "anon"));
        assert!(!within_root("anon/", "anon"));
        assert!(!within_root("app/research", "anon"));
        assert!(!within_root("anon//research", "anon"));
    }
}
