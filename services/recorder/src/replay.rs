use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use moqt::{
    RequestErrorCode, Session, SessionEvent,
    fetch::{FetchHandler, FetchParams, JoinPoint, resolve_fetch_range, resolve_joining_fetch},
    object::{ContentExists, GroupOrder, Location},
    subscribe::FilterType,
};
use serde_json::Value;
use tokio::{
    sync::mpsc,
    task::{AbortHandle, JoinError, JoinHandle, JoinSet},
};

use crate::{
    archive_reader::{StoredRecording, StoredTrack, decode_object},
    relay::{RelayConnection, ServiceToken},
};

/// Shorter than the Cloud Run request timeout (60 minutes, its maximum), so a
/// client opens the next answer before Cloud Run ends the current one.
const RENEW_AFTER: Duration = Duration::from_secs(45 * 60);
const MAX_FETCHES: usize = 4;

/// Serves immutable recorded tracks while a [`ReplayLease`] exists.
pub(crate) struct ReplayTask {
    handle: JoinHandle<()>,
    session: Arc<Session>,
    description: Value,
    lease_sender: mpsc::WeakSender<()>,
}

/// The replay ends when the last lease is dropped, which closes the channel;
/// nothing is sent on it.
pub(crate) struct ReplayLease {
    _lease_sender: mpsc::Sender<()>,
}

impl ReplayLease {
    pub(crate) fn new(lease_sender: mpsc::Sender<()>) -> Self {
        Self {
            _lease_sender: lease_sender,
        }
    }
}

impl ReplayTask {
    /// The replay ends once the returned lease and every lease taken with
    /// [`Self::lease`] are dropped.
    pub(crate) async fn run(
        recording: StoredRecording,
        relay: &str,
        verify_certificate: bool,
        token: &ServiceToken,
    ) -> Result<(Self, ReplayLease)> {
        let namespace = &recording
            .tracks
            .first()
            .context("recording has no tracks")?
            .source_namespace;
        let connection =
            RelayConnection::connect_service(relay, verify_certificate, namespace, token).await?;
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
        let (lease_sender, lease_receiver) = mpsc::channel(1);
        let handle = tokio::spawn(async move {
            if let Err(error) =
                serve(&connection.session, Arc::new(recording), lease_receiver).await
            {
                tracing::warn!(%error, "recording replay stopped");
            }
            connection.close().await;
        });
        let task = Self {
            handle,
            session,
            description,
            lease_sender: lease_sender.downgrade(),
        };
        Ok((task, ReplayLease::new(lease_sender)))
    }

    /// `None` once the last lease is gone and the replay is ending.
    pub(crate) fn lease(&self) -> Option<ReplayLease> {
        self.lease_sender.upgrade().map(ReplayLease::new)
    }

    pub(crate) fn is_active(&self) -> bool {
        !self.handle.is_finished()
    }

    pub(crate) fn description(&self) -> Value {
        let mut description = self.description.clone();
        description["renew_after_seconds"] = RENEW_AFTER.as_secs().into();
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
    mut lease_receiver: mpsc::Receiver<()>,
) -> Result<()> {
    let mut fetches = JoinSet::new();
    let mut active: HashMap<u64, AbortHandle> = HashMap::new();
    // The track of each SUBSCRIBE a Joining Fetch may join, by request id.
    let mut subscriptions: HashMap<u64, Arc<StoredTrack>> = HashMap::new();
    loop {
        let event = tokio::select! {
            None = lease_receiver.recv() => break,
            _ = session.closed() => break,
            Some(result) = fetches.join_next(), if !fetches.is_empty() => {
                fetch_finished(result, &mut active);
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
                let (track, start, end) = match fetch_target(&recording, &subscriptions, &handler) {
                    Ok(target) => target,
                    Err((code, reason)) => {
                        handler.error(code, reason).await?;
                        continue;
                    }
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
                        subscriptions.insert(handler.request_id(), track.clone());
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
                subscriptions.remove(&handler.subscribe_id());
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

/// The track and the `[start, end)` range a FETCH asks for, as a live
/// publisher resolves them: a Joining Fetch against the Largest Location its
/// SUBSCRIBE was answered with (draft-14 §9.16.2), a Standalone Fetch against
/// the track's last object (§9.16.1). `Err` holds the FETCH_ERROR.
fn fetch_target(
    recording: &StoredRecording,
    subscriptions: &HashMap<u64, Arc<StoredTrack>>,
    handler: &FetchHandler,
) -> Result<(Arc<StoredTrack>, Location, Location), (RequestErrorCode, String)> {
    if let Some((request_id, joining_start)) = handler.joining() {
        let track = subscriptions.get(&request_id).ok_or((
            RequestErrorCode::InvalidJoiningRequestId,
            "no subscription has the joining request id".to_string(),
        ))?;
        // Every subscription kept here was answered with the track's last object.
        let (start, end) =
            resolve_joining_fetch(joining_start, JoinPoint::Largest(Some(track.last))).map_err(
                |error| {
                    (
                        error
                            .request_error_code()
                            .unwrap_or(RequestErrorCode::InvalidRange),
                        error.to_string(),
                    )
                },
            )?;
        return Ok((track.clone(), start, end));
    }
    let FetchParams::Standalone {
        track_namespace,
        track_name,
        start_location,
        end_location,
    } = &handler.fetch.fetch_params
    else {
        unreachable!("only a Standalone Fetch joins no subscription");
    };
    let namespace = track_namespace.join("/");
    let track = recording
        .tracks
        .iter()
        .find(|track| track.namespace == namespace && track.name == *track_name)
        .ok_or((
            RequestErrorCode::TrackDoesNotExist,
            "no such recorded track".to_string(),
        ))?;
    let end =
        resolve_fetch_range(*start_location, *end_location, Some(track.last)).map_err(|_| {
            (
                RequestErrorCode::InvalidRange,
                "invalid recording range".to_string(),
            )
        })?;
    Ok((track.clone(), *start_location, end))
}

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
    use crate::test_support::{connected_sessions, stored_object, stored_recording};
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
    async fn leases_keep_replay_alive_until_the_last_one_is_dropped_and_fetch_preserves_objects() {
        // Arrange
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
        let (store, recording) = stored_recording(vec![("video", objects.clone())]).await;
        let recording = Arc::new(recording);
        let namespace = recording.tracks[0].namespace.clone();
        let corrupt_key =
            object_store::path::Path::from(recording.tracks[0].chunks[0]["data"].as_str().unwrap());
        let pair = tokio::time::timeout(Duration::from_secs(5), connected_sessions())
            .await
            .expect("connect sessions");
        let server = pair.server.clone();
        let (lease_sender, lease_receiver) = mpsc::channel(1);
        let other_lease_sender = lease_sender.clone();
        let serving = tokio::spawn(async move { serve(&server, recording, lease_receiver).await });
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
        store
            .object_store()
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
        drop(lease_sender);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !serving.is_finished(),
            "one remaining lease must keep replay alive"
        );
        drop(other_lease_sender);
        tokio::time::timeout(Duration::from_secs(5), serving)
            .await
            .expect("server close")
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), pair.client.close())
            .await
            .expect("client close");
    }

    #[tokio::test]
    async fn a_joining_fetch_returns_the_groups_before_the_last_object_as_a_live_publisher_would() {
        // Arrange
        let objects = vec![
            stored_object(1, 0, "g1"),
            stored_object(5, 0, "g5"),
            stored_object(7, 0, "g7a"),
            stored_object(7, 1, "g7b"),
        ];
        let (_store, recording) = stored_recording(vec![("video", objects.clone())]).await;
        let namespace = recording.tracks[0].namespace.clone();
        let pair = connected_sessions().await;
        let server = pair.server.clone();
        let (_lease_sender, lease_receiver) = mpsc::channel(1);
        tokio::spawn(async move { serve(&server, Arc::new(recording), lease_receiver).await });
        let mut subscriber = pair.client.session.subscriber();
        let subscription = subscriber
            .subscribe(
                namespace,
                "video".into(),
                SubscribeOption {
                    filter_type: FilterType::LargestObject,
                    ..SubscribeOption::default()
                },
            )
            .await
            .unwrap();
        let fetch = async |subscriber: &mut moqt::Subscriber, handle| {
            let mut receiver = subscriber.accept_fetch_receiver(&handle).await.unwrap();
            let mut payloads = Vec::new();
            while let Fetch::Object(object) = receiver.receive().await.unwrap() {
                payloads.push(object.payload);
            }
            payloads
        };

        // Act
        let mut joined = Vec::new();
        for joining_start in [0, 2] {
            let handle = subscriber
                .fetch_relative_joining(&subscription, joining_start, FetchOption::default())
                .await
                .unwrap();
            joined.push(
                tokio::time::timeout(Duration::from_secs(5), fetch(&mut subscriber, handle))
                    .await
                    .unwrap(),
            );
        }

        // Assert: offset 0 takes the last group; offset 2 starts at group 5, two ids before it, though group 6 was never published
        assert_eq!(joined, [vec!["g7a", "g7b"], vec!["g5", "g7a", "g7b"]]);
        pair.client.close().await;
    }
}
