use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use moqt::{
    RequestErrorCode, Session, SessionEvent,
    fetch::{FetchHandler, FetchParams, resolve_fetch_range},
    object::{ContentExists, GroupOrder, Location},
    subscribe::FilterType,
};
use serde_json::Value;
use tokio::{
    task::{AbortHandle, JoinError, JoinHandle, JoinSet},
    time::Instant,
};

use crate::{
    archive_reader::{StoredRecording, StoredTrack, decode_object},
    relay::RelayConnection,
};

const REPLAY_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MAX_FETCHES: usize = 4;

/// Serves immutable recorded tracks until no subscription or FETCH remains
/// for the idle timeout.
pub(crate) struct ReplayTask {
    handle: JoinHandle<()>,
    session: Arc<Session>,
    description: Value,
}

impl ReplayTask {
    pub(crate) async fn run(
        recording: StoredRecording,
        relay: &str,
        verify_certificate: bool,
    ) -> Result<Self> {
        let namespace = &recording
            .tracks
            .first()
            .context("recording has no tracks")?
            .source_namespace;
        let connection =
            RelayConnection::connect_service(relay, verify_certificate, namespace).await?;
        let mut namespaces = HashSet::new();
        for track in &recording.tracks {
            if namespaces.insert(track.namespace.clone())
                && let Err(error) = connection
                    .session
                    .publisher()
                    .publish_namespace(track.namespace.clone())
                    .await
            {
                connection.close().await;
                return Err(error.into());
            }
        }
        let description = recording.description();
        let session = connection.session.clone();
        let handle = tokio::spawn(async move {
            if let Err(error) = serve(
                &connection.session,
                Arc::new(recording),
                REPLAY_IDLE_TIMEOUT,
            )
            .await
            {
                tracing::warn!(%error, "recording replay stopped");
            }
            connection.close().await;
        });
        Ok(Self {
            handle,
            session,
            description,
        })
    }

    pub(crate) fn is_active(&self) -> bool {
        !self.handle.is_finished()
    }

    pub(crate) fn description(&self) -> Value {
        let mut description = self.description.clone();
        description["idle_timeout_seconds"] = REPLAY_IDLE_TIMEOUT.as_secs().into();
        description
    }
}

impl Drop for ReplayTask {
    fn drop(&mut self) {
        self.session
            .close_with_error(moqt::TerminationErrorCode::NoError, "replay ended");
        self.handle.abort();
    }
}

async fn serve(
    session: &Arc<Session>,
    recording: Arc<StoredRecording>,
    idle_timeout: Duration,
) -> Result<()> {
    let mut fetches = JoinSet::new();
    let mut active: HashMap<u64, AbortHandle> = HashMap::new();
    let mut subscribers = HashSet::new();
    let mut idle_since = Instant::now();
    loop {
        let event = tokio::select! {
            _ = tokio::time::sleep_until(idle_since + idle_timeout), if subscribers.is_empty() && active.is_empty() => break,
            _ = session.closed() => break,
            Some(result) = fetches.join_next(), if !fetches.is_empty() => {
                fetch_finished(result, &mut active);
                idle_since = Instant::now();
                continue;
            },
            event = session.receive_event() => event?,
        };
        match event {
            SessionEvent::Fetch(handler) => {
                if active.len() >= MAX_FETCHES {
                    handler
                        .error(
                            RequestErrorCode::InternalError,
                            "too many recording fetches".into(),
                        )
                        .await?;
                    continue;
                }
                let FetchParams::Standalone {
                    track_namespace,
                    track_name,
                    start_location,
                    end_location,
                } = &handler.fetch.fetch_params
                else {
                    handler
                        .error(
                            RequestErrorCode::NotSupported,
                            "recordings use standalone FETCH".into(),
                        )
                        .await?;
                    continue;
                };
                let namespace = track_namespace.join("/");
                let Some(track) = recording
                    .tracks
                    .iter()
                    .find(|track| track.namespace == namespace && track.name == *track_name)
                    .cloned()
                else {
                    handler
                        .error(
                            RequestErrorCode::TrackDoesNotExist,
                            "no such recorded track".into(),
                        )
                        .await?;
                    continue;
                };
                let start = *start_location;
                let Ok(end) = resolve_fetch_range(start, *end_location, Some(track.last)) else {
                    handler
                        .error(
                            RequestErrorCode::InvalidRange,
                            "invalid recording range".into(),
                        )
                        .await?;
                    continue;
                };
                let request_id = handler.request_id;
                let session = session.clone();
                let recording = recording.clone();
                let task = fetches.spawn(async move {
                    (
                        request_id,
                        send_fetch(&session, recording, track, handler, start, end).await,
                    )
                });
                active.insert(request_id, task);
            }
            SessionEvent::FetchCancel(handler) => {
                if let Some(task) = active.remove(&handler.request_id()) {
                    task.abort();
                    idle_since = Instant::now();
                }
            }
            SessionEvent::Subscribe(handler) => {
                match recording.tracks.iter().find(|track| {
                    track.namespace == handler.track_namespace && track.name == handler.track_name
                }) {
                    Some(track) if handler.filter_type == FilterType::LargestObject => {
                        handler
                            .ok(
                                0,
                                ContentExists::True {
                                    location: track.last,
                                },
                            )
                            .await?;
                        subscribers.insert(handler.request_id());
                    }
                    Some(_) => {
                        handler
                            .error(
                                RequestErrorCode::NotSupported,
                                "recorded objects are available through FETCH".into(),
                            )
                            .await?
                    }
                    None => {
                        handler
                            .error(
                                RequestErrorCode::TrackDoesNotExist,
                                "no such recorded track".into(),
                            )
                            .await?
                    }
                }
            }
            SessionEvent::Unsubscribe(handler) => {
                if subscribers.remove(&handler.subscribe_id()) {
                    idle_since = Instant::now();
                }
            }
            SessionEvent::TrackStatus(handler) => {
                match recording.tracks.iter().find(|track| {
                    track.namespace == handler.track_namespace()
                        && track.name == handler.track_name()
                }) {
                    Some(track) => {
                        handler
                            .ok(
                                0,
                                ContentExists::True {
                                    location: track.last,
                                },
                            )
                            .await?
                    }
                    None => {
                        handler
                            .error(
                                RequestErrorCode::TrackDoesNotExist,
                                "no such recorded track".into(),
                            )
                            .await?
                    }
                }
            }
            SessionEvent::Disconnected() => break,
            _ => {}
        }
    }
    Ok(())
}

/// Frees the slot of a FETCH task that ended, including one that panicked.
fn fetch_finished(
    result: Result<(u64, Result<()>), JoinError>,
    active: &mut HashMap<u64, AbortHandle>,
) {
    match result {
        Ok((request_id, result)) => {
            active.remove(&request_id);
            if let Err(error) = result {
                tracing::warn!(request_id, %error, "recording FETCH failed");
            }
        }
        Err(error) => {
            // A cancelled FETCH was already removed; a panicked one is removed here.
            active.retain(|_, task| task.id() != error.id());
            if error.is_panic() {
                tracing::error!(%error, "recording FETCH panicked");
            }
        }
    }
}

async fn send_fetch(
    session: &Session,
    recording: Arc<StoredRecording>,
    track: Arc<StoredTrack>,
    handler: FetchHandler,
    start: Location,
    end: Location,
) -> Result<()> {
    let entries = match prepare_fetch(&recording, &track, start, end).await {
        Ok(entries) => entries,
        Err(error) => {
            handler
                .error(
                    RequestErrorCode::MalformedTrack,
                    "recording data failed integrity validation".into(),
                )
                .await?;
            return Err(error);
        }
    };
    if entries.is_empty() {
        handler
            .error(
                RequestErrorCode::NoObjects,
                "no recorded objects in this range".into(),
            )
            .await?;
        return Ok(());
    }
    handler.ok(end == track.last.next(), end).await?;
    let sender = session
        .publisher()
        .create_fetch_stream(handler.request_id)
        .await?;
    let result = async {
        let mut entries = entries;
        if handler.group_order == GroupOrder::Descending {
            entries.sort_by(|a, b| {
                b.location
                    .group_id
                    .cmp(&a.location.group_id)
                    .then(a.location.object_id.cmp(&b.location.object_id))
            });
        }
        let mut cached_chunk = None;
        let mut data = bytes::Bytes::new();
        for entry in entries {
            if cached_chunk != Some(entry.chunk) {
                data = recording.data(&track.chunks[entry.chunk]).await?;
                cached_chunk = Some(entry.chunk);
            }
            sender
                .send_object(&decode_object(&entry.value, &data)?)
                .await?;
        }
        anyhow::Ok(())
    }
    .await;
    match result {
        Ok(()) => sender.close().await,
        Err(error) => {
            let _ = sender
                .reset(moqt::object::DataStreamResetCode::InternalError)
                .await;
            Err(error)
        }
    }
}

async fn prepare_fetch(
    recording: &StoredRecording,
    track: &StoredTrack,
    start: Location,
    end: Location,
) -> Result<Vec<crate::archive_reader::StoredEntry>> {
    let entries = recording.entries(track, start, end).await?;
    let chunks: HashSet<usize> = entries.iter().map(|entry| entry.chunk).collect();
    // Check the data before FETCH_OK: a failure after it resets the stream, which can discard the FETCH header too.
    for chunk in chunks {
        recording.data(&track.chunks[chunk]).await?;
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        archive_chunks::ChunkWriter, archive_receiver::Event, store::RecordingStore,
        test_support::connected_sessions,
    };
    use bytes::Bytes;
    use moqt::{
        fetch::{Fetch, FetchOption},
        object::{ExtensionHeaders, ForwardingPreference, Object, ObjectStatus},
        subscribe::SubscribeOption,
    };
    use object_store::{ObjectStoreExt, PutPayload};

    #[tokio::test]
    async fn a_panicked_fetch_frees_its_slot() {
        // Arrange
        let mut fetches = JoinSet::new();
        let mut active = HashMap::new();
        let panicked = fetches.spawn(async { panic!("FETCH task panicked") });
        let running = fetches.spawn(std::future::pending());
        active.insert(1, panicked);
        active.insert(2, running);

        // Act
        let result = fetches.join_next().await.unwrap();
        fetch_finished(result, &mut active);

        // Assert: only the panicked FETCH leaves the active set
        assert_eq!(active.keys().copied().collect::<Vec<_>>(), [2]);
    }

    #[tokio::test]
    async fn a_failed_fetch_frees_its_slot() {
        // Arrange
        let mut fetches = JoinSet::new();
        let mut active = HashMap::new();
        active.insert(
            1,
            fetches.spawn(async { (1, Err(anyhow::anyhow!("object store unavailable"))) }),
        );

        // Act
        let result = fetches.join_next().await.unwrap();
        fetch_finished(result, &mut active);

        // Assert
        assert!(active.is_empty());
    }

    #[tokio::test]
    async fn subscribers_keep_replay_alive_until_the_last_one_leaves_and_fetch_preserves_objects() {
        // Arrange
        let store = RecordingStore::open(&url::Url::parse("memory:///").unwrap()).unwrap();
        let backend = store.object_store();
        let prefix = store.recording_prefix("id");
        let mut writer = ChunkWriter::new(
            backend.clone(),
            prefix.clone().join("archive"),
            "anon/room/alice".into(),
        );
        let make = |group_id, object_id, payload, status| Object {
            location: Location {
                group_id,
                object_id,
            },
            forwarding: ForwardingPreference::Subgroup { subgroup_id: 2 },
            publisher_priority: 17,
            status,
            extension_headers: ExtensionHeaders::from_immutable_extensions(vec![
                Bytes::from_static(b"unknown"),
            ]),
            payload,
            ends_group: false,
        };
        let objects = vec![
            make(
                3,
                1,
                Bytes::from(vec![8; 4 * 1024 * 1024]),
                ObjectStatus::Normal,
            ),
            make(1, 0, Bytes::from_static(b"first"), ObjectStatus::Normal),
            make(3, 2, Bytes::new(), ObjectStatus::EndOfTrack),
        ];
        for object in &objects {
            writer
                .push("video", &Event::Object(object.clone()))
                .await
                .unwrap();
        }
        let key = writer.finish().await.unwrap();
        backend.put(&prefix.join("manifest.json"), PutPayload::from(serde_json::json!({ "version": 1, "format": "moq", "members": [{ "namespace": "anon/room/alice", "camera": [key.as_ref()], "screen_share": [] }] }).to_string())).await.unwrap();
        let recording = Arc::new(
            StoredRecording::open(&store, "id", "anon/room")
                .await
                .unwrap(),
        );
        let namespace = recording.tracks[0].namespace.clone();
        let corrupt_key =
            object_store::path::Path::from(recording.tracks[0].chunks[0]["data"].as_str().unwrap());
        let pair = tokio::time::timeout(Duration::from_secs(5), connected_sessions())
            .await
            .expect("connect sessions");
        let server = pair.server.clone();
        let serving =
            tokio::spawn(
                async move { serve(&server, recording, Duration::from_millis(200)).await },
            );
        let start = Location {
            group_id: 0,
            object_id: 0,
        };
        let end = Location {
            group_id: 4,
            object_id: 0,
        };
        let mut subscriber = pair.client.session.subscriber();

        // Act
        let presence = subscriber
            .subscribe(
                namespace.clone(),
                "video".into(),
                SubscribeOption {
                    forward: false,
                    ..SubscribeOption::default()
                },
            )
            .await
            .unwrap();
        let other_presence = subscriber
            .subscribe(
                namespace.clone(),
                "video".into(),
                SubscribeOption {
                    forward: false,
                    ..SubscribeOption::default()
                },
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !serving.is_finished(),
            "active subscribers must prevent idle expiry"
        );
        let handle = tokio::time::timeout(
            Duration::from_secs(5),
            subscriber.fetch(
                namespace.clone(),
                "video".into(),
                start,
                end,
                FetchOption::default(),
            ),
        )
        .await
        .expect("first FETCH response")
        .unwrap();
        let mut fetched = tokio::time::timeout(
            Duration::from_secs(5),
            subscriber.accept_fetch_receiver(&handle),
        )
        .await
        .expect("first FETCH stream")
        .unwrap();
        let received = tokio::time::timeout(Duration::from_secs(5), async {
            let mut objects = Vec::new();
            while let Fetch::Object(object) = fetched.receive().await.unwrap() {
                objects.push(object);
            }
            objects
        })
        .await
        .unwrap();

        // Assert
        assert_eq!(
            received,
            [objects[1].clone(), objects[0].clone(), objects[2].clone()]
        );
        backend
            .put(&corrupt_key, PutPayload::from("damaged"))
            .await
            .unwrap();
        let rejected = tokio::time::timeout(
            Duration::from_secs(5),
            subscriber.fetch(
                namespace.clone(),
                "video".into(),
                start,
                end,
                FetchOption::default(),
            ),
        )
        .await
        .expect("second FETCH response");
        assert!(rejected.is_err());
        subscriber.unsubscribe(&presence).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !serving.is_finished(),
            "one remaining subscriber must keep replay alive"
        );
        subscriber.unsubscribe(&other_presence).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), serving)
            .await
            .expect("server close")
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), pair.client.close())
            .await
            .expect("client close");
    }
}
