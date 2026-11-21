use std::{
    collections::HashMap,
    hash::{BuildHasher, RandomState},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use auth_token::{TEMPORARY_APP_ID_PREFIX, VtsClient};
use hyper::HeaderMap;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, OnceCell};

use crate::{
    ServeArgs,
    api_auth::{self, AuthError},
    archive_reader::StoredRecording,
    recording::Layout,
    relay::ServiceToken,
    replay::{RENEW_AFTER, ReplayLease, ReplayTask},
    room_state::{CompositeState, MemberStatus, RecordingFormat, RoomRecord, RoomState},
    state_sync_task::{HEARTBEAT_INTERVAL, now_ms},
    store::RecordingStore,
    token_refresh_task::TokenRenewal,
};

/// A recording job that has not written its state for this long is treated
/// as dead.
const STALE_AFTER: Duration = HEARTBEAT_INTERVAL.saturating_mul(3);
/// How long a URL from `file_url` stays valid.
const FILE_URL_LIFETIME: Duration = Duration::from_secs(15 * 60);
/// Cap on `app_recordings`; the newest are kept.
const MAX_APP_RECORDINGS: usize = 100;

#[derive(Debug)]
pub(crate) enum PublishRejection {
    NotReady,
    /// An MP4 recording keeps neither the catalog nor the media timeline a
    /// replay serves.
    NotMoq,
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
    vts: Option<VtsClient>,
    /// A cell is reserved under the lock and filled outside it, so opening
    /// one recording does not block requests for the others.
    replays: Mutex<HashMap<String, Arc<OnceCell<ReplayTask>>>>,
}

impl Recordings {
    pub(crate) fn new(config: ServeArgs, store: RecordingStore) -> Self {
        let vts = if config.insecure_skip_auth {
            None
        } else {
            // clap requires `--auth-vts-url` unless `--insecure-skip-auth`.
            let url = config
                .auth_vts_url
                .clone()
                .expect("--auth-vts-url without --insecure-skip-auth");
            Some(VtsClient::new(url))
        };
        Self {
            config,
            store,
            vts,
            replays: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn authorize(
        &self,
        headers: &HeaderMap,
        namespace: &str,
    ) -> Result<(), AuthError> {
        let Some(vts) = &self.vts else {
            return Ok(());
        };
        api_auth::authorize(headers, vts, namespace).await
    }

    pub(crate) async fn namespace(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .store
            .read_state(id)
            .await?
            .map(|record| record.namespace_prefix))
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
        let token = self.service_token(&record.namespace_prefix, bearer);
        loop {
            let cell = {
                let mut replays = self.replays.lock().await;
                replays.retain(|_, cell| cell.get().is_none_or(ReplayTask::is_active));
                if !replays.contains_key(id) && replays.len() >= self.config.max_replays {
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
                        &self.config.relay,
                        !self.config.insecure_skip_tls_verify,
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

    pub(crate) async fn get(&self, id: &str) -> Result<Option<Value>> {
        match self.store.read_state(id).await? {
            Some(record) => Ok(Some(self.view(record).await?)),
            None => Ok(None),
        }
    }

    pub(crate) async fn list(&self, namespace_prefix: &str) -> Result<Value> {
        let mut recordings = Vec::new();
        for id in self.store.room_recording_ids(namespace_prefix).await? {
            if let Some(record) = self.store.read_state(&id).await? {
                recordings.push(self.view(record).await?);
            }
        }
        Ok(json!({ "recordings": recordings }))
    }

    /// The MP4 recordings of `app_id` that have a file in the store, newest
    /// first, each with its files.
    pub(crate) async fn app_recordings(&self, app_id: &str) -> Result<Value> {
        let mut recordings = Vec::new();
        for id in self.store.app_recording_ids(app_id).await? {
            if recordings.len() >= MAX_APP_RECORDINGS {
                break;
            }
            let Some(record) = self.store.read_state(&id).await? else {
                continue;
            };
            if record.format != RecordingFormat::Mp4
                || record.namespace_prefix.split('/').next() != Some(app_id)
            {
                continue;
            }
            let files: Vec<Value> = self
                .store
                .recording_files(&record.namespace_prefix, &id)
                .await?
                .into_iter()
                .map(|file| json!({ "name": file.name, "size_bytes": file.size_bytes }))
                .collect();
            let mut view = self.view(record).await?;
            view["files"] = Value::Array(files);
            recordings.push(view);
        }
        Ok(json!({ "recordings": recordings }))
    }

    /// A signed URL that reads the MP4 file `name` of recording `id` from the
    /// store for `FILE_URL_LIFETIME`; `None` for an unknown recording or
    /// file.
    pub(crate) async fn file_url(&self, id: &str, name: &str) -> Result<Option<Value>> {
        let Some(record) = self.store.read_state(id).await? else {
            return Ok(None);
        };
        let url = self
            .store
            .file_url(&record.namespace_prefix, id, name, FILE_URL_LIFETIME)
            .await?;
        Ok(url.map(|url| json!({ "url": url.as_str() })))
    }

    /// The token a replay of the room presents: the caller's, renewed at the
    /// temporary-apps service when the room is a temporary app's.
    fn service_token(&self, namespace_prefix: &str, bearer: Option<&str>) -> ServiceToken {
        let temporary = namespace_prefix.starts_with(TEMPORARY_APP_ID_PREFIX);
        ServiceToken {
            token: bearer.map(str::to_string),
            renewal: self
                .config
                .temporary_apps_url
                .clone()
                .filter(|_| temporary && bearer.is_some())
                .map(TokenRenewal::Issuer),
        }
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
        Ok(serde_json::to_value(RecordingView {
            requested_at_ms: minted_ms(&record.id),
            id: record.id,
            namespace_prefix: record.namespace_prefix,
            layout: record.layout,
            format: record.format,
            state,
            members: record.members,
            composite,
        })?)
    }
}

/// A recording as the recordings API returns it.
#[derive(Serialize)]
struct RecordingView {
    id: String,
    requested_at_ms: Option<u64>,
    namespace_prefix: String,
    layout: Layout,
    format: RecordingFormat,
    #[serde(flatten)]
    state: RoomState,
    members: Vec<MemberStatus>,
    composite: Option<CompositeState>,
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
pub(crate) fn recording_id() -> String {
    let random = RandomState::new().hash_one(now_ms()) & 0xff_ffff;
    format!("{}-{random:06x}", now_ms())
}

/// When the recording job made `id`, from its `<Unix ms>` part.
fn minted_ms(id: &str) -> Option<u64> {
    id.split('-').next().and_then(|ms| ms.parse().ok())
}

#[cfg(test)]
mod tests {
    use object_store::{ObjectStoreExt, PutPayload};

    use super::*;
    use crate::test_support::{TEMPORARY_APPS_URL, recordings};

    #[tokio::test]
    async fn refuses_to_replay_an_mp4_recording() {
        // Arrange
        let recordings = recordings(1);
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

    #[test]
    fn a_replay_presents_the_callers_token_renewed_only_for_a_temporary_app() {
        // Arrange
        let recordings = recordings(1);

        // Act
        let temporary = recordings.service_token("temp-abc/room", Some("jwt"));
        let registered = recordings.service_token("app/room", Some("jwt"));

        // Assert
        assert_eq!(temporary.token.as_deref(), Some("jwt"));
        assert!(
            matches!(&temporary.renewal, Some(TokenRenewal::Issuer(url)) if url == TEMPORARY_APPS_URL)
        );
        assert_eq!(registered.token.as_deref(), Some("jwt"));
        assert!(registered.renewal.is_none());
    }

    #[tokio::test]
    async fn lists_only_the_apps_mp4_recordings_with_their_files() {
        // Arrange
        let recordings = recordings(1);
        let record = |id: &str, namespace_prefix: &str, format| RoomRecord {
            id: id.to_string(),
            namespace_prefix: namespace_prefix.to_string(),
            layout: Layout::Combined,
            format,
            composite: false,
            state: RoomState::Finished,
            members: Vec::new(),
            updated_at_ms: now_ms(),
        };
        for (id, prefix, format) in [
            ("1700000000000-000001", "app/room", RecordingFormat::Mp4),
            ("1700000001000-000002", "app/room", RecordingFormat::Moq),
            ("1700000002000-000003", "other/room", RecordingFormat::Mp4),
        ] {
            recordings
                .store
                .write_state(&record(id, prefix, format))
                .await
                .unwrap();
            recordings
                .store
                .object_store()
                .put(
                    &recordings.store.recording_prefix(prefix, id).join("a.mp4"),
                    PutPayload::from_static(b"123"),
                )
                .await
                .unwrap();
        }

        // Act
        let listed = recordings.app_recordings("app").await.unwrap();

        // Assert
        let listed = listed["recordings"].as_array().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["id"], "1700000000000-000001");
        assert_eq!(listed[0]["requested_at_ms"], 1_700_000_000_000_u64);
        assert_eq!(
            listed[0]["files"],
            json!([{ "name": "a.mp4", "size_bytes": 3 }])
        );
    }

    #[tokio::test]
    async fn a_recording_view_keeps_its_json_shape() {
        // Arrange: composited, but the composite job has not written its state yet
        let recordings = recordings(1);
        let record = RoomRecord {
            id: "1700000000000-000001".to_string(),
            namespace_prefix: "app/room".to_string(),
            layout: Layout::PerTrack,
            format: RecordingFormat::Mp4,
            composite: true,
            state: RoomState::Finished,
            members: vec![MemberStatus {
                namespace: "app/room/a".to_string(),
                started_at_ms: 1_700_000_000_100,
                finished: true,
                error: Some("relay gone".to_string()),
            }],
            updated_at_ms: now_ms(),
        };

        // Act
        let view = recordings.view(record).await.unwrap().to_string();

        // Assert
        assert_eq!(
            view,
            concat!(
                r#"{"composite":{"error":null,"state":"waiting"},"error":null,"format":"mp4","#,
                r#""id":"1700000000000-000001","layout":"per-track","members":[{"error":"relay gone","#,
                r#""finished":true,"namespace":"app/room/a","started_at_ms":1700000000100}],"#,
                r#""namespace_prefix":"app/room","requested_at_ms":1700000000000,"state":"finished"}"#,
            )
        );
    }
}
