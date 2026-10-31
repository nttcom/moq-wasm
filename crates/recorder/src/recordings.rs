use std::{
    collections::HashMap,
    hash::{BuildHasher, RandomState},
    time::Duration,
};

use anyhow::Result;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{
    ServeArgs,
    archive_reader::StoredRecording,
    launcher::{Job, Launcher},
    recording::Layout,
    replay::ReplayTask,
    room_state::{CompositeState, MemberStatus, RecordingFormat, RoomRecord, RoomState},
    state_sync_task::{HEARTBEAT_INTERVAL, now_ms},
    store::RecordingStore,
};

/// A recording job that has not written its state for this long is treated
/// as dead.
const STALE_AFTER: Duration = HEARTBEAT_INTERVAL.saturating_mul(3);

#[derive(Debug)]
pub(crate) enum StartRejection {
    /// The prefix is outside `namespace_root` or names no room.
    OutsideRoot,
    AlreadyRecording,
    TooManyRecordings,
    CompositeDisabled,
    CompositeRequiresMp4,
    /// The store or the launcher failed.
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for StartRejection {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

#[derive(Debug)]
pub(crate) enum PublishRejection {
    NotReady,
    Capacity,
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for PublishRejection {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

/// Recording state belongs to the store and independent jobs. Replay
/// publications belong to this API process for their bounded lifetime.
pub(crate) struct Recordings {
    config: ServeArgs,
    store: RecordingStore,
    launcher: Launcher,
    replays: Mutex<HashMap<String, ReplayTask>>,
}

impl Recordings {
    pub(crate) fn new(config: ServeArgs, store: RecordingStore, launcher: Launcher) -> Self {
        Self {
            config,
            store,
            launcher,
            replays: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn authorize(
        &self,
        headers: &hyper::HeaderMap,
        namespace: &str,
        write: bool,
    ) -> Result<(), crate::api_auth::AuthError> {
        crate::api_auth::authorize(
            headers,
            self.config.auth_vts_url.as_deref(),
            namespace,
            write,
        )
        .await
    }

    pub(crate) async fn namespace(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .store
            .read_state(id)
            .await?
            .map(|record| record.namespace_prefix))
    }

    pub(crate) fn namespace_root(&self) -> &str {
        &self.config.namespace_root
    }

    pub(crate) async fn publish(&self, id: &str) -> Result<Option<Value>, PublishRejection> {
        let Some(record) = self.store.read_state(id).await? else {
            return Ok(None);
        };
        if record.state != RoomState::Finished {
            return Err(PublishRejection::NotReady);
        }
        let mut replays = self.replays.lock().await;
        replays.retain(|_, replay| replay.is_active());
        if !replays.contains_key(id) {
            if replays.len() >= self.config.max_recordings {
                return Err(PublishRejection::Capacity);
            }
            let archive = StoredRecording::open(&self.store, id, &record.namespace_prefix).await?;
            let replay = ReplayTask::run(
                archive,
                &self.config.room.relay,
                !self.config.room.insecure_skip_tls_verify,
            )
            .await?;
            replays.insert(id.to_string(), replay);
        }
        let mut description = replays[id].description();
        description["recording_id"] = id.into();
        description["relay"] = self.config.room.relay.clone().into();
        description["namespace_prefix"] =
            format!("{}/recordings/{id}", record.namespace_prefix).into();
        Ok(Some(description))
    }

    pub(crate) async fn start(
        &self,
        namespace_prefix: &str,
        layout: Layout,
        composite: bool,
        format: RecordingFormat,
    ) -> Result<Value, StartRejection> {
        if !within_root(namespace_prefix, &self.config.namespace_root) {
            return Err(StartRejection::OutsideRoot);
        }
        if composite && !self.config.composite {
            return Err(StartRejection::CompositeDisabled);
        }
        if composite && format == RecordingFormat::Moq {
            return Err(StartRejection::CompositeRequiresMp4);
        }
        let id = recording_id();
        if !self.lock_room(namespace_prefix, &id).await? {
            return Err(StartRejection::AlreadyRecording);
        }
        // Counted after taking the room, and not atomically: concurrent
        // requests can all pass and exceed the limit by their number.
        if self.other_active_rooms(&id).await? >= self.config.max_recordings {
            self.store.unlock_room(namespace_prefix, &id).await?;
            return Err(StartRejection::TooManyRecordings);
        }
        let record = RoomRecord {
            id: id.clone(),
            namespace_prefix: namespace_prefix.to_string(),
            layout,
            format,
            composite,
            state: RoomState::Starting,
            members: Vec::new(),
            updated_at_ms: now_ms(),
        };
        self.store.write_state(&record).await?;
        self.store.add_to_room(namespace_prefix, &id).await?;
        if let Err(error) = self
            .launcher
            .launch(Job::Record, self.record_args(&id))
            .await
        {
            tracing::error!(%id, error = format!("{error:#}"), "cannot launch the recording job");
            let failed = RoomRecord {
                state: RoomState::Failed(format!("cannot launch the recording job: {error:#}")),
                ..record
            };
            self.store.write_state(&failed).await?;
            self.store.unlock_room(namespace_prefix, &id).await?;
            return Err(StartRejection::Internal(error));
        }
        tracing::info!(%id, namespace_prefix, ?layout, composite, "recording requested");
        Ok(self.view(record).await?)
    }

    /// `None` for an unknown recording; a finished one is answered as is.
    pub(crate) async fn stop(&self, id: &str) -> Result<Option<Value>> {
        let Some(record) = self.store.read_state(id).await? else {
            return Ok(None);
        };
        if record.state.is_active() {
            self.store.request_stop(id).await?;
        }
        Ok(Some(self.view(record).await?))
    }

    pub(crate) async fn get(&self, id: &str) -> Result<Option<Value>> {
        match self.store.read_state(id).await? {
            Some(record) => Ok(Some(self.view(record).await?)),
            None => Ok(None),
        }
    }

    pub(crate) async fn list(&self, namespace_prefix: Option<&str>) -> Result<Value> {
        let ids = match namespace_prefix {
            Some(prefix) => self.store.room_recording_ids(prefix).await?,
            None => self.store.recording_ids().await?,
        };
        let mut recordings = Vec::new();
        for id in ids {
            if let Some(record) = self.store.read_state(&id).await? {
                if record.namespace_prefix.split('/').next() != Some(&self.config.namespace_root) {
                    continue;
                }
                recordings.push(self.view(record).await?);
            }
        }
        Ok(json!({ "recordings": recordings }))
    }

    /// Takes the room, first releasing a lock whose recording is no longer
    /// active.
    async fn lock_room(&self, namespace_prefix: &str, id: &str) -> Result<bool> {
        if self.store.lock_room(namespace_prefix, id).await? {
            return Ok(true);
        }
        let Some(holder) = self.store.room_lock(namespace_prefix).await? else {
            return self.store.lock_room(namespace_prefix, id).await;
        };
        if self.holds_active_recording(&holder).await? {
            return Ok(false);
        }
        self.store.lock_room(namespace_prefix, id).await
    }

    /// The number of rooms held by an active recording other than `id`.
    /// Releases each lock left by a recording that ended or whose job died.
    async fn other_active_rooms(&self, id: &str) -> Result<usize> {
        let mut active = 0;
        for holder in self.store.lock_holders().await? {
            if holder != id && self.holds_active_recording(&holder).await? {
                active += 1;
            }
        }
        Ok(active)
    }

    async fn holds_active_recording(&self, holder: &str) -> Result<bool> {
        let Some(record) = self.store.read_state(holder).await? else {
            return Ok(false);
        };
        if effective_state(&record, now_ms()).is_active() {
            return Ok(true);
        }
        tracing::info!(%holder, prefix = %record.namespace_prefix, "releasing the room of a recording that ended");
        self.store
            .unlock_room(&record.namespace_prefix, holder)
            .await?;
        Ok(false)
    }

    fn record_args(&self, id: &str) -> Vec<String> {
        let mut args = vec![
            "--id".to_string(),
            id.to_string(),
            "--store".to_string(),
            self.config.store.to_string(),
            "--output-dir".to_string(),
            self.config.output_dir.display().to_string(),
        ];
        args.extend(self.config.room.to_args());
        args.extend(self.config.launcher.to_args());
        args
    }

    async fn view(&self, record: RoomRecord) -> Result<Value> {
        let state = effective_state(&record, now_ms());
        let composite = if record.composite {
            Some(match self.store.read_composite(&record.id).await? {
                Some(composite) => composite,
                None if matches!(state, RoomState::Failed(_)) => {
                    CompositeState::Skipped("the recording failed".to_string())
                }
                None => CompositeState::Waiting,
            })
        } else {
            None
        };
        let (state_name, error) = state.parts();
        Ok(json!({
            "id": record.id,
            "namespace_prefix": record.namespace_prefix,
            "layout": record.layout.name(),
            "format": record.format.name(),
            "state": state_name,
            "error": error,
            "members": record.members.iter().map(MemberStatus::to_json).collect::<Vec<_>>(),
            "composite": composite.as_ref().map(CompositeState::to_json),
        }))
    }
}

fn effective_state(record: &RoomRecord, now_ms: u64) -> RoomState {
    let silent_ms = now_ms.saturating_sub(record.updated_at_ms);
    if record.state.is_active() && silent_ms > STALE_AFTER.as_millis() as u64 {
        RoomState::Failed("the recording job stopped".to_string())
    } else {
        record.state.clone()
    }
}

/// `<Unix ms>-<6 hex digits>`: sorts by start time and does not collide
/// between API instances.
fn recording_id() -> String {
    let random = RandomState::new().hash_one(now_ms()) & 0xff_ffff;
    format!("{}-{random:06x}", now_ms())
}

/// `<root>/<room>[/...]` with no empty element.
fn within_root(namespace_prefix: &str, root: &str) -> bool {
    let parts: Vec<&str> = namespace_prefix.split('/').collect();
    parts.len() >= 2 && parts[0] == root && parts.iter().all(|part| !part.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::recordings;

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

    #[tokio::test]
    async fn starts_a_recording_once_per_room_and_stops_it_through_the_store() {
        // Arrange
        let recordings = recordings(2, true);

        // Act
        let started = recordings
            .start(
                "anon/research",
                Layout::Combined,
                true,
                RecordingFormat::Mp4,
            )
            .await
            .unwrap();
        let again = recordings
            .start(
                "anon/research",
                Layout::Combined,
                false,
                RecordingFormat::Mp4,
            )
            .await;
        let id = started["id"].as_str().unwrap().to_string();
        recordings.stop(&id).await.unwrap();
        let listed = recordings.list(Some("anon/research")).await.unwrap();

        // Assert
        assert_eq!(started["state"], "starting");
        assert_eq!(started["composite"]["state"], "waiting");
        assert!(matches!(again, Err(StartRejection::AlreadyRecording)));
        assert!(recordings.store.stop_requested(&id).await.unwrap());
        assert_eq!(listed["recordings"][0]["id"], id.as_str());
    }

    #[tokio::test]
    async fn takes_over_the_room_of_a_recording_whose_job_died() {
        // Arrange: a recording that has not written its state for 100 s
        let recordings = recordings(2, false);
        let dead = RoomRecord {
            id: "1700000000000-000001".to_string(),
            namespace_prefix: "anon/research".to_string(),
            layout: Layout::Combined,
            format: RecordingFormat::Mp4,
            composite: false,
            state: RoomState::Recording,
            members: Vec::new(),
            updated_at_ms: now_ms() - 100_000,
        };
        recordings.store.write_state(&dead).await.unwrap();
        recordings
            .store
            .lock_room("anon/research", &dead.id)
            .await
            .unwrap();

        // Act
        let dead_view = recordings.get(&dead.id).await.unwrap().unwrap();
        let started = recordings
            .start(
                "anon/research",
                Layout::Combined,
                false,
                RecordingFormat::Mp4,
            )
            .await
            .unwrap();

        // Assert
        assert_eq!(dead_view["state"], "failed");
        assert_eq!(dead_view["error"], "the recording job stopped");
        assert_eq!(started["state"], "starting");
    }

    #[tokio::test]
    async fn refuses_a_room_beyond_the_limit() {
        // Arrange
        let recordings = recordings(1, false);
        recordings
            .start("anon/first", Layout::Combined, false, RecordingFormat::Mp4)
            .await
            .unwrap();

        // Act
        let second = recordings
            .start("anon/second", Layout::Combined, false, RecordingFormat::Mp4)
            .await;

        // Assert
        assert!(matches!(second, Err(StartRejection::TooManyRecordings)));
        assert_eq!(recordings.store.lock_holders().await.unwrap().len(), 1);
    }
}
