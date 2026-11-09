use std::{
    collections::HashMap,
    hash::{BuildHasher, RandomState},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use auth_token::TEMPORARY_APP_ID_PREFIX;
use hyper::HeaderMap;
use serde_json::{Value, json};
use tokio::sync::{Mutex, OnceCell};

use crate::{
    ServeArgs,
    api_auth::{self, AuthError},
    archive_reader::StoredRecording,
    launcher::{Job, Launcher},
    recording::Layout,
    relay::{ServiceToken, TemporaryToken},
    replay::{RENEW_AFTER, ReplayLease, ReplayTask},
    room_state::{CompositeState, MemberStatus, RecordingFormat, RoomRecord, RoomState},
    state_sync_task::{HEARTBEAT_INTERVAL, now_ms},
    store::RecordingStore,
};

/// A recording job that has not written its state for this long is treated
/// as dead.
const STALE_AFTER: Duration = HEARTBEAT_INTERVAL.saturating_mul(3);

#[derive(Debug)]
pub(crate) enum StartRejection {
    /// The prefix names no room under `namespace_root`, nor, with a token
    /// mint and a bearer token, under a temporary app's root.
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
    /// An MP4 recording keeps neither the catalog nor the media timeline a
    /// replay serves.
    NotMoq,
    /// The recording's room is not one this server can replay, as in
    /// `StartRejection::OutsideRoot`.
    OutsideRoot,
    Capacity,
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for PublishRejection {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

pub(crate) struct Publication {
    pub(crate) description: Value,
    pub(crate) lease: ReplayLease,
}

/// Recording state belongs to the store and independent jobs. Replay
/// publications belong to this API process while a lease on them exists.
pub(crate) struct Recordings {
    config: ServeArgs,
    store: RecordingStore,
    launcher: Launcher,
    /// A cell is reserved under the lock and filled outside it, so opening
    /// one recording does not block requests for the others.
    replays: Mutex<HashMap<String, Arc<OnceCell<ReplayTask>>>>,
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
        headers: &HeaderMap,
        namespace: &str,
        write: bool,
    ) -> Result<(), AuthError> {
        api_auth::authorize(
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

    pub(crate) async fn publish(
        &self,
        id: &str,
        bearer: Option<&str>,
    ) -> Result<Option<Publication>, PublishRejection> {
        let Some(record) = self.store.read_state(id).await? else {
            return Ok(None);
        };
        if record.state != RoomState::Finished {
            return Err(PublishRejection::NotReady);
        }
        if record.format != RecordingFormat::Moq {
            return Err(PublishRejection::NotMoq);
        }
        let token = self
            .service_token(&record.namespace_prefix, bearer)
            .ok_or(PublishRejection::OutsideRoot)?;
        loop {
            let cell = {
                let mut replays = self.replays.lock().await;
                replays.retain(|_, cell| cell.get().is_none_or(ReplayTask::is_active));
                if !replays.contains_key(id) && replays.len() >= self.config.max_recordings {
                    return Err(PublishRejection::Capacity);
                }
                Arc::clone(replays.entry(id.to_string()).or_default())
            };
            let mut first_lease = None;
            let replay = cell
                .get_or_try_init(|| async {
                    let archive =
                        StoredRecording::open(&self.store, id, &record.namespace_prefix).await?;
                    let (replay, lease) = ReplayTask::run(
                        archive,
                        &self.config.room.relay,
                        !self.config.room.insecure_skip_tls_verify,
                        &token,
                    )
                    .await?;
                    first_lease = Some(lease);
                    anyhow::Ok(replay)
                })
                .await;
            let (lease, started_at_ms) = match replay {
                Ok(replay) => (
                    first_lease.or_else(|| replay.lease()),
                    replay.started_at_ms(),
                ),
                Err(error) => {
                    self.forget_replay(id, &cell).await;
                    return Err(error.into());
                }
            };
            let Some(lease) = lease else {
                // The replay is ending: its last lease is gone but its task
                // has not finished, so `retain` kept it. Forget it; the next
                // turn starts a new replay.
                self.forget_replay(id, &cell).await;
                continue;
            };
            let description = json!({
                "namespace_prefix": format!("{}/recordings/{id}", record.namespace_prefix),
                "renew_after_seconds": RENEW_AFTER.as_secs(),
                "started_at_ms": started_at_ms,
            });
            return Ok(Some(Publication { description, lease }));
        }
    }

    async fn forget_replay(&self, id: &str, cell: &Arc<OnceCell<ReplayTask>>) {
        let mut replays = self.replays.lock().await;
        if replays
            .get(id)
            .is_some_and(|current| Arc::ptr_eq(current, cell))
        {
            replays.remove(id);
        }
    }

    pub(crate) async fn start(
        &self,
        namespace_prefix: &str,
        layout: Layout,
        composite: bool,
        format: RecordingFormat,
        bearer: Option<&str>,
    ) -> Result<Value, StartRejection> {
        let Some(token) = self.service_token(namespace_prefix, bearer) else {
            return Err(StartRejection::OutsideRoot);
        };
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
        // Counted after taking the room, so that of concurrent requests the one
        // that locks last counts all the others.
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
            .launch(Job::Record, self.record_args(&id, &token))
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
                // Without a prefix the list is the public root's alone: a
                // temporary app's rooms are listed only to a caller
                // authorized for their prefix.
                let listed = match namespace_prefix {
                    Some(_) => self.accepts_root(&record.namespace_prefix),
                    None => {
                        record.namespace_prefix.split('/').next()
                            == Some(&self.config.namespace_root)
                    }
                };
                if !listed {
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
            // A request between taking its lock and writing its state.
            return Ok(minted_within_stale_window(holder, now_ms()));
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

    /// The token a recording or replay of the room presents: the recorder's
    /// own under `namespace_root`, the caller's under a temporary app's root
    /// when a token mint is configured to renew it. `None` when the room is
    /// outside both, or a temporary app's room comes without a bearer token.
    fn service_token(&self, namespace_prefix: &str, bearer: Option<&str>) -> Option<ServiceToken> {
        if within_root(namespace_prefix, &self.config.namespace_root) {
            return Some(ServiceToken::Environment);
        }
        let issuer_url = self.config.token_mint_url.as_ref()?;
        let root = namespace_prefix.split('/').next()?;
        if !root.starts_with(TEMPORARY_APP_ID_PREFIX) || !within_root(namespace_prefix, root) {
            return None;
        }
        Some(ServiceToken::Temporary(TemporaryToken {
            token: bearer?.to_string(),
            issuer_url: issuer_url.clone(),
        }))
    }

    fn accepts_root(&self, namespace_prefix: &str) -> bool {
        let root = namespace_prefix.split('/').next().unwrap_or_default();
        root == self.config.namespace_root
            || (self.config.token_mint_url.is_some() && root.starts_with(TEMPORARY_APP_ID_PREFIX))
    }

    fn record_args(&self, id: &str, token: &ServiceToken) -> Vec<String> {
        let mut args = vec![
            "--id".to_string(),
            id.to_string(),
            "--store".to_string(),
            self.config.store.to_string(),
            "--output-dir".to_string(),
            self.config.output_dir.display().to_string(),
        ];
        if let ServiceToken::Temporary(temporary) = token {
            args.extend([
                "--temporary-token".to_string(),
                temporary.token.clone(),
                "--token-mint-url".to_string(),
                temporary.issuer_url.clone(),
            ]);
        }
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

fn minted_within_stale_window(id: &str, now_ms: u64) -> bool {
    let minted_ms = id.split('-').next().and_then(|ms| ms.parse::<u64>().ok());
    minted_ms.is_some_and(|ms| now_ms.saturating_sub(ms) < STALE_AFTER.as_millis() as u64)
}

/// `<root>/<room>[/...]` with no empty element.
fn within_root(namespace_prefix: &str, root: &str) -> bool {
    let parts: Vec<&str> = namespace_prefix.split('/').collect();
    parts.len() >= 2 && parts[0] == root && parts.iter().all(|part| !part.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TOKEN_MINT_URL, recordings};

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
                None,
            )
            .await
            .unwrap();
        let again = recordings
            .start(
                "anon/research",
                Layout::Combined,
                false,
                RecordingFormat::Mp4,
                None,
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
    async fn refuses_to_replay_an_mp4_recording() {
        // Arrange
        let recordings = recordings(1, false);
        let finished = RoomRecord {
            id: "1700000000000-000001".to_string(),
            namespace_prefix: "anon/research".to_string(),
            layout: Layout::Combined,
            format: RecordingFormat::Mp4,
            composite: false,
            state: RoomState::Finished,
            members: Vec::new(),
            updated_at_ms: now_ms(),
        };
        recordings.store.write_state(&finished).await.unwrap();

        // Act
        let published = recordings.publish(&finished.id, None).await;

        // Assert
        assert!(matches!(published, Err(PublishRejection::NotMoq)));
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
                None,
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
            .start(
                "anon/first",
                Layout::Combined,
                false,
                RecordingFormat::Mp4,
                None,
            )
            .await
            .unwrap();

        // Act
        let second = recordings
            .start(
                "anon/second",
                Layout::Combined,
                false,
                RecordingFormat::Mp4,
                None,
            )
            .await;

        // Assert
        assert!(matches!(second, Err(StartRejection::TooManyRecordings)));
        assert_eq!(recordings.store.lock_holders().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_room_locked_but_not_yet_written_counts_toward_the_limit() {
        // Arrange
        let recordings = recordings(1, false);
        recordings
            .store
            .lock_room("anon/first", &recording_id())
            .await
            .unwrap();

        // Act
        let second = recordings
            .start(
                "anon/second",
                Layout::Combined,
                false,
                RecordingFormat::Mp4,
                None,
            )
            .await;

        // Assert
        assert!(matches!(second, Err(StartRejection::TooManyRecordings)));
        assert_eq!(recordings.store.lock_holders().await.unwrap().len(), 1);
    }

    #[test]
    fn only_an_id_minted_within_the_stale_window_is_fresh() {
        // Arrange
        let now = 1_700_000_100_000;

        // Act / Assert
        assert!(minted_within_stale_window("1700000050000-00abcd", now));
        assert!(!minted_within_stale_window("1700000000000-00abcd", now));
        assert!(!minted_within_stale_window("not-an-id", now));
    }

    #[test]
    fn a_temporary_app_room_is_recorded_with_the_callers_token() {
        // Arrange
        let recordings = recordings(1, false);

        // Act
        let token = recordings.service_token("temp-abc/room", Some("jwt"));

        // Assert
        let Some(ServiceToken::Temporary(temporary)) = token else {
            panic!("expected the caller's token, got {token:?}");
        };
        assert_eq!(
            (temporary.token.as_str(), temporary.issuer_url.as_str()),
            ("jwt", TOKEN_MINT_URL)
        );
    }

    #[test]
    fn only_the_root_and_temporary_apps_with_a_token_are_recorded() {
        // Arrange
        let recordings = recordings(1, false);

        // Act / Assert
        assert!(matches!(
            recordings.service_token("anon/room", None),
            Some(ServiceToken::Environment)
        ));
        assert!(recordings.service_token("temp-abc/room", None).is_none());
        assert!(recordings.service_token("temp-abc", Some("jwt")).is_none());
        assert!(recordings.service_token("app/room", Some("jwt")).is_none());
    }

    #[tokio::test]
    async fn the_job_of_a_temporary_app_room_gets_the_token_and_the_issuer() {
        // Arrange
        let recordings = recordings(1, false);
        let token = recordings
            .service_token("temp-abc/room", Some("jwt"))
            .unwrap();

        // Act
        let args = recordings.record_args("1", &token);

        // Assert
        let joined = args.join(" ");
        assert!(joined.contains(&format!(
            "--temporary-token jwt --token-mint-url {TOKEN_MINT_URL}"
        )));
    }

    #[tokio::test]
    async fn a_temporary_app_room_is_listed_only_under_its_prefix() {
        // Arrange
        let recordings = recordings(2, false);
        recordings
            .start(
                "temp-abc/room",
                Layout::Combined,
                false,
                RecordingFormat::Mp4,
                Some("jwt"),
            )
            .await
            .unwrap();

        // Act
        let everything = recordings.list(None).await.unwrap();
        let the_room = recordings.list(Some("temp-abc/room")).await.unwrap();

        // Assert
        assert_eq!(everything["recordings"].as_array().unwrap().len(), 0);
        assert_eq!(the_room["recordings"].as_array().unwrap().len(), 1);
    }
}
