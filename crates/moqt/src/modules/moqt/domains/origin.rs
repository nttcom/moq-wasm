use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use tokio::sync::Mutex;

use crate::modules::moqt::control_plane::error_codes::RequestErrorCode;

use crate::{
    Session, TrackWriter, TransportSendError,
    fetch::FetchHandler,
    fetch::FetchParams,
    modules::executor,
    object::{ContentExists, ExtensionHeaders, Location},
    publish::PublishOption,
    subscribe::{SubscribeHandler, TrackStatusHandler},
};

mod fetch_serve_task;
mod object_cache;
mod object_numbering;

pub use object_cache::Retention;
pub use object_numbering::{GroupBoundary, Placement};

use fetch_serve_task::FetchServeTask;
use object_cache::{CachedObject, FetchRange, ObjectCache};
use object_numbering::ObjectNumbering;

const SUBSCRIBE_EXPIRES_MS: u64 = 1_000_000;
const FETCH_PUBLISHER_PRIORITY: u8 = 128;
const FETCH_SUBGROUP_ID: u64 = 0;

type TrackKey = (String, String);

pub struct OutgoingObject {
    pub group: GroupBoundary,
    pub extension_headers: ExtensionHeaders,
    pub payload: Bytes,
}

impl OutgoingObject {
    pub fn plain(group: GroupBoundary, payload: Bytes) -> Self {
        Self {
            group,
            extension_headers: ExtensionHeaders::default(),
            payload,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Sent(Placement),
    /// The peer stopped the subgroup stream; the object stays cached and the
    /// track resumes at the next group.
    Stopped(Placement),
    /// Dropped because no group is open for a `Within` object, or numbered and
    /// cached only: the track has no writer, or its writer waits for the next
    /// group to start.
    NotSent,
}

/// Objects are numbered and cached from the first one handed over, whether or
/// not the track is published yet, so the cache and the live stream agree on
/// ids; a writer created while a group is open therefore sends from the next
/// group on. Content Exists reports what was actually sent.
#[derive(Clone)]
pub struct Origin {
    session: Arc<Session>,
    state: Arc<Mutex<OriginState>>,
}

#[derive(Default)]
struct OriginState {
    tracks: HashMap<TrackKey, OriginTrack>,
    subscriptions: HashMap<u64, TrackKey>,
    fetches: HashMap<u64, FetchServeTask>,
}

impl OriginState {
    fn track_mut(&mut self, key: &TrackKey) -> Result<&mut OriginTrack> {
        self.tracks.get_mut(key).with_context(|| {
            format!(
                "track {}/{} is not registered with the origin",
                key.0, key.1
            )
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Publication {
    Unpublished,
    /// A SUBSCRIBE the peer sent is being served.
    Subscribed,
    /// A PUBLISH this endpoint sent is being served.
    Published,
    /// `published` tells whether the released subscription came from PUBLISH.
    Released {
        published: bool,
    },
}

struct OriginTrack {
    numbering: ObjectNumbering,
    cache: ObjectCache,
    publication: Publication,
    writer: Option<TrackWriter>,
    largest_sent: Option<Location>,
}

impl OriginTrack {
    fn new(first_group_id: u64, retention: Retention) -> Self {
        Self {
            numbering: ObjectNumbering::new(first_group_id),
            cache: ObjectCache::new(retention),
            publication: Publication::Unpublished,
            writer: None,
            largest_sent: None,
        }
    }

    fn serve(&mut self, publication: Publication, writer: TrackWriter) {
        self.publication = publication;
        self.writer = Some(writer);
    }

    fn release(&mut self) {
        self.publication = Publication::Released {
            published: self.publication == Publication::Published,
        };
        self.writer = None;
    }

    fn serving(&self) -> bool {
        matches!(
            self.publication,
            Publication::Subscribed | Publication::Published
        )
    }
}

impl Origin {
    // On wasm32 the browser transport is !Send, but the state is shared with
    // the native build, where the tasks run on a multi-threaded executor.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            state: Arc::new(Mutex::new(OriginState::default())),
        }
    }

    pub async fn ensure_track(
        &self,
        namespace: &str,
        track_name: &str,
        first_group_id: u64,
        retention: Retention,
    ) {
        self.state
            .lock()
            .await
            .tracks
            .entry(track_key(namespace, track_name))
            .or_insert_with(|| OriginTrack::new(first_group_id, retention));
    }

    /// A no-op while a subscription to the track is served, and once a
    /// PUBLISH of it has been released: the peer unsubscribed from what this
    /// endpoint offered. A track released after a SUBSCRIBE is published
    /// again.
    pub async fn publish(&self, namespace: &str, track_name: &str) -> Result<()> {
        let key = track_key(namespace, track_name);
        let mut state = self.state.lock().await;
        let track = state.track_mut(&key)?;
        if !matches!(
            track.publication,
            Publication::Unpublished | Publication::Released { published: false }
        ) {
            return Ok(());
        }
        let next_group_id = track.numbering.next_group_id();
        let publisher = self.session.publisher();
        let subscription = publisher
            .publish(
                namespace.to_string(),
                track_name.to_string(),
                PublishOption::default(),
            )
            .await
            .with_context(|| format!("publish {namespace}/{track_name}"))?;
        state.track_mut(&key)?.serve(
            Publication::Published,
            TrackWriter::new(publisher.create_stream(&subscription), next_group_id),
        );
        state.subscriptions.insert(subscription.request_id(), key);
        tracing::info!(
            namespace,
            track_name,
            track_alias = subscription.track_alias(),
            "track published"
        );
        Ok(())
    }

    pub async fn write(
        &self,
        namespace: &str,
        track_name: &str,
        object: OutgoingObject,
    ) -> Result<WriteOutcome> {
        let key = track_key(namespace, track_name);
        let (placement, mut writer) = {
            let mut state = self.state.lock().await;
            let track = state.track_mut(&key)?;
            let Some(placement) = track.numbering.place(object.group)? else {
                return Ok(WriteOutcome::NotSent);
            };
            track.cache.insert(
                CachedObject {
                    location: placement.location,
                    extension_headers: object.extension_headers.clone(),
                    payload: object.payload.clone(),
                },
                executor::now(),
            );
            let Some(writer) = track.writer.take() else {
                return Ok(WriteOutcome::NotSent);
            };
            (placement, writer)
        };
        let result = send_placed(&mut writer, placement, object).await;
        self.settle(&key, writer, placement, result).await
    }

    pub async fn write_group(
        &self,
        namespace: &str,
        track_name: &str,
        payload: Bytes,
    ) -> Result<WriteOutcome> {
        let key = track_key(namespace, track_name);
        let (placement, mut writer) = {
            let mut state = self.state.lock().await;
            let track = state.track_mut(&key)?;
            let placement = track
                .numbering
                .place(GroupBoundary::Next)?
                .context("a new group always places its first object")?;
            track.cache.insert(
                CachedObject {
                    location: placement.location,
                    extension_headers: ExtensionHeaders::default(),
                    payload: payload.clone(),
                },
                executor::now(),
            );
            let Some(writer) = track.writer.take() else {
                return Ok(WriteOutcome::NotSent);
            };
            if writer.next_group_id() != placement.location.group_id {
                let next_group_id = writer.next_group_id();
                track.writer = Some(writer);
                bail!(
                    "writer is at group {next_group_id} but the cache placed the object at group {}",
                    placement.location.group_id
                );
            }
            (placement, writer)
        };
        let result = writer
            .write_group(payload)
            .await
            .context("send group")
            .map(|()| true);
        self.settle(&key, writer, placement, result).await
    }

    pub async fn accept_subscribe(&self, handler: SubscribeHandler) -> Result<()> {
        let key = (handler.track_namespace.clone(), handler.track_name.clone());
        let content_exists = {
            let mut state = self.state.lock().await;
            content_exists(state.track_mut(&key)?.largest_sent)
        };
        let track_alias = handler
            .ok(SUBSCRIBE_EXPIRES_MS, content_exists)
            .await
            .context("accept SUBSCRIBE")?;
        let subscription = handler.into_subscription(track_alias);
        let mut state = self.state.lock().await;
        let track = state.track_mut(&key)?;
        let next_group_id = track.numbering.next_group_id();
        track.serve(
            Publication::Subscribed,
            TrackWriter::new(
                self.session.publisher().create_stream(&subscription),
                next_group_id,
            ),
        );
        state.subscriptions.insert(handler.request_id(), key);
        tracing::info!(
            namespace = %handler.track_namespace,
            track_name = %handler.track_name,
            track_alias,
            "SUBSCRIBE accepted"
        );
        Ok(())
    }

    /// Returns the released track's `(namespace, name)`; its numbering and
    /// cache are kept.
    pub async fn release(&self, request_id: u64) -> Option<(String, String)> {
        let mut state = self.state.lock().await;
        let key = state.subscriptions.remove(&request_id)?;
        if let Some(track) = state.tracks.get_mut(&key) {
            track.release();
        }
        Some(key)
    }

    pub async fn serve_fetch(&self, handler: FetchHandler) -> Result<()> {
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
                    "only standalone FETCH is served".to_string(),
                )
                .await?;
            return Ok(());
        };
        let key = (track_namespace.join("/"), track_name.clone());
        let mut state = self.state.lock().await;
        let range = state
            .tracks
            .get(&key)
            .map(|track| track.cache.resolve(*start_location, *end_location));
        let (code, reason) = match range {
            None => (RequestErrorCode::TrackDoesNotExist, "track not published"),
            Some(FetchRange::InvalidRange) => (RequestErrorCode::InvalidRange, "invalid range"),
            Some(FetchRange::NoObjects) => {
                (RequestErrorCode::NoObjects, "no cached objects in range")
            }
            Some(FetchRange::Serve {
                objects,
                end_location,
            }) => {
                tracing::info!(
                    request_id = handler.request_id,
                    namespace = %key.0,
                    track_name = %key.1,
                    objects = objects.len(),
                    "FETCH served from the origin cache"
                );
                let request_id = handler.request_id;
                let task = FetchServeTask::run(
                    self.session.clone(),
                    Arc::downgrade(&self.state),
                    handler,
                    objects,
                    end_location,
                );
                state.fetches.insert(request_id, task);
                return Ok(());
            }
        };
        drop(state);
        handler.error(code, reason.to_string()).await?;
        Ok(())
    }

    pub async fn cancel_fetch(&self, request_id: u64) {
        if self
            .state
            .lock()
            .await
            .fetches
            .remove(&request_id)
            .is_some()
        {
            tracing::info!(request_id, "FETCH_CANCEL received; response aborted");
        }
    }

    pub async fn answer_track_status(&self, handler: TrackStatusHandler) -> Result<()> {
        let key = (
            handler.track_namespace().to_string(),
            handler.track_name().to_string(),
        );
        let largest_sent = self
            .state
            .lock()
            .await
            .tracks
            .get(&key)
            .map(|track| track.largest_sent);
        match largest_sent {
            Some(largest_sent) => handler.ok(0, content_exists(largest_sent)).await?,
            None => {
                handler
                    .error(
                        RequestErrorCode::TrackDoesNotExist,
                        format!("{} is not a track of this origin", key.1),
                    )
                    .await?
            }
        }
        Ok(())
    }

    async fn settle(
        &self,
        key: &TrackKey,
        writer: TrackWriter,
        placement: Placement,
        result: Result<bool>,
    ) -> Result<WriteOutcome> {
        let mut state = self.state.lock().await;
        let track = state.track_mut(key)?;
        if track.writer.is_none() && track.serving() {
            track.writer = Some(writer);
        }
        match result {
            Ok(true) => {
                track.largest_sent = Some(placement.location);
                Ok(WriteOutcome::Sent(placement))
            }
            Ok(false) => Ok(WriteOutcome::NotSent),
            Err(error) if TransportSendError::is_stopped_by_peer(&error) => {
                tracing::info!(
                    namespace = %key.0,
                    track_name = %key.1,
                    "subscriber stopped a subgroup; the track resumes at the next group"
                );
                Ok(WriteOutcome::Stopped(placement))
            }
            Err(error) => Err(error),
        }
    }
}

fn track_key(namespace: &str, track_name: &str) -> TrackKey {
    (namespace.to_string(), track_name.to_string())
}

fn content_exists(largest_sent: Option<Location>) -> ContentExists {
    match largest_sent {
        Some(location) => ContentExists::True { location },
        None => ContentExists::False,
    }
}

async fn send_placed(
    writer: &mut TrackWriter,
    placement: Placement,
    object: OutgoingObject,
) -> Result<bool> {
    if placement.starts_group {
        writer
            .start_group_at(placement.location.group_id)
            .await
            .context("start group")?;
    } else if writer.current_group_id() != Some(placement.location.group_id) {
        return Ok(false);
    }
    writer
        .write_with_extension_headers(object.payload, object.extension_headers)
        .await
        .context("send subgroup object")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::{future::Future, time::Duration};

    use bytes::Bytes;

    use super::*;
    use crate::{
        RequestFailure, SessionEvent, Subscription, TrackObject,
        fetch::{FetchHandle, FetchObjectField, FetchOption},
        modules::test_support::{
            HANDSHAKE_TIMEOUT, accept_publish, read_fetch_objects, rejection,
            spawn_connected_dual_sessions, subscribed_track_reader,
        },
        object::GroupOrder,
        subscribe::{FilterType, SubscribeOption},
    };

    const NAMESPACE: &str = "ns";
    const TRACK: &str = "track";
    const FIRST_GROUP_ID: u64 = 7;
    const RETENTION: Retention = Retention::For(Duration::from_secs(60));

    struct Peers {
        origin: Origin,
        client: Arc<Session>,
        server: Session,
    }

    async fn origin_with_track(name: &str, retention: Retention) -> Peers {
        let (client, server) = spawn_connected_dual_sessions(name).await;
        let client = Arc::new(client);
        let origin = Origin::new(client.clone());
        origin
            .ensure_track(NAMESPACE, TRACK, FIRST_GROUP_ID, retention)
            .await;
        Peers {
            origin,
            client,
            server,
        }
    }

    async fn within<F: Future>(future: F) -> F::Output {
        tokio::time::timeout(HANDSHAKE_TIMEOUT, future)
            .await
            .expect("the exchange timed out")
    }

    fn location(group_id: u64, object_id: u64) -> Location {
        Location {
            group_id,
            object_id,
        }
    }

    fn plain(group: GroupBoundary, payload: &'static [u8]) -> OutgoingObject {
        OutgoingObject::plain(group, Bytes::from_static(payload))
    }

    fn sent(group_id: u64, object_id: u64, starts_group: bool) -> WriteOutcome {
        WriteOutcome::Sent(Placement {
            location: location(group_id, object_id),
            starts_group,
        })
    }

    fn ids(objects: &[FetchObjectField]) -> Vec<(u64, u64)> {
        objects
            .iter()
            .map(|object| (object.group_id, object.object_id))
            .collect()
    }

    async fn publish(peers: &Peers) -> Subscription {
        let (published, accepted) = within(async {
            tokio::join!(
                peers.origin.publish(NAMESPACE, TRACK),
                accept_publish(&peers.server)
            )
        })
        .await;
        published.unwrap();
        accepted
    }

    async fn unsubscribe(peers: &Peers, subscription: &Subscription) -> Option<(String, String)> {
        let subscriber = peers.server.subscriber();
        let (unsubscribed, released) = within(async {
            tokio::join!(subscriber.unsubscribe(subscription), async {
                let SessionEvent::Unsubscribe(handler) =
                    peers.client.receive_event().await.unwrap()
                else {
                    panic!("expected UNSUBSCRIBE from the relay");
                };
                peers.origin.release(handler.subscribe_id()).await
            })
        })
        .await;
        unsubscribed.unwrap();
        released
    }

    async fn subscribe(peers: &Peers) -> Subscription {
        let option = SubscribeOption {
            filter_type: FilterType::NextGroupStart,
            ..SubscribeOption::default()
        };
        let mut subscriber = peers.server.subscriber();
        let (subscription, ()) = within(async {
            tokio::join!(
                subscriber.subscribe(NAMESPACE.to_string(), TRACK.to_string(), option),
                async {
                    let SessionEvent::Subscribe(handler) =
                        peers.client.receive_event().await.unwrap()
                    else {
                        panic!("expected SUBSCRIBE from the relay");
                    };
                    peers.origin.accept_subscribe(handler).await.unwrap();
                }
            )
        })
        .await;
        subscription.unwrap()
    }

    async fn fetch(
        peers: &Peers,
        track_name: &str,
        start: Location,
        end: Location,
        group_order: GroupOrder,
    ) -> Result<(FetchHandle, Vec<FetchObjectField>)> {
        let mut subscriber = peers.server.subscriber();
        let option = FetchOption {
            group_order,
            ..FetchOption::default()
        };
        let (handle, ()) = within(async {
            tokio::join!(
                subscriber.fetch(
                    NAMESPACE.to_string(),
                    track_name.to_string(),
                    start,
                    end,
                    option
                ),
                async {
                    let SessionEvent::Fetch(handler) = peers.client.receive_event().await.unwrap()
                    else {
                        panic!("expected FETCH from the relay");
                    };
                    peers.origin.serve_fetch(handler).await.unwrap();
                }
            )
        })
        .await;
        let handle = handle?;
        let mut receiver = subscriber.accept_fetch_receiver(&handle).await?;
        let objects = within(read_fetch_objects(&mut receiver)).await;
        Ok((handle, objects))
    }

    async fn track_status(
        peers: &Peers,
        track_name: &str,
    ) -> std::result::Result<crate::wire::TrackStatusOk, RequestFailure> {
        let subscriber = peers.server.subscriber();
        let (status, ()) = within(async {
            tokio::join!(
                subscriber.track_status(NAMESPACE.to_string(), track_name.to_string(), vec![]),
                async {
                    let SessionEvent::TrackStatus(handler) =
                        peers.client.receive_event().await.unwrap()
                    else {
                        panic!("expected TRACK_STATUS from the relay");
                    };
                    peers.origin.answer_track_status(handler).await.unwrap();
                }
            )
        })
        .await;
        status
    }

    async fn next_object(peers: &Peers, subscription: &Subscription) -> TrackObject {
        let mut reader = subscribed_track_reader(&peers.server, subscription).await;
        within(reader.next_object()).await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn write_caches_before_publish_and_sends_from_the_next_group_after_it() {
        // Arrange
        let peers = origin_with_track("origin-publish", RETENTION).await;
        let origin = &peers.origin;

        // Act
        let before_publish = origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
            .await
            .unwrap();
        let accepted = publish(&peers).await;
        let mid_group = origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Within, b"b"))
            .await
            .unwrap();
        let next_group = origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"c"))
            .await
            .unwrap();
        let received = next_object(&peers, &accepted).await;

        // Assert
        assert_eq!(before_publish, WriteOutcome::NotSent);
        assert_eq!(mid_group, WriteOutcome::NotSent);
        assert_eq!(next_group, sent(FIRST_GROUP_ID + 1, 0, true));
        assert_eq!(
            (received.group_id, received.object_id, received.payload),
            (FIRST_GROUP_ID + 1, 0, Bytes::from_static(b"c"))
        );
        let (handle, cached) = fetch(
            &peers,
            TRACK,
            location(FIRST_GROUP_ID, 0),
            location(FIRST_GROUP_ID + 2, 0),
            GroupOrder::Ascending,
        )
        .await
        .unwrap();
        assert_eq!(
            ids(&cached),
            [
                (FIRST_GROUP_ID, 0),
                (FIRST_GROUP_ID, 1),
                (FIRST_GROUP_ID + 1, 0)
            ]
        );
        assert_eq!(handle.end_location, location(FIRST_GROUP_ID + 1, 1));
    }

    #[tokio::test]
    async fn accepted_subscribe_starts_at_the_next_group_and_status_reports_what_was_sent() {
        // Arrange
        let peers = origin_with_track("origin-subscribe", RETENTION).await;
        let origin = &peers.origin;
        origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
            .await
            .unwrap();

        // Act
        let subscription = subscribe(&peers).await;
        let mid_group = origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Within, b"b"))
            .await
            .unwrap();
        let next_group = origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"c"))
            .await
            .unwrap();
        let received = next_object(&peers, &subscription).await;
        let status = track_status(&peers, TRACK).await.unwrap();
        let unknown = track_status(&peers, "other").await;

        // Assert
        assert_eq!(subscription.content_exists(), ContentExists::False);
        assert_eq!(mid_group, WriteOutcome::NotSent);
        assert_eq!(next_group, sent(FIRST_GROUP_ID + 1, 0, true));
        assert_eq!(
            (received.group_id, received.object_id),
            (FIRST_GROUP_ID + 1, 0)
        );
        assert_eq!(
            status.content_exists,
            ContentExists::True {
                location: location(FIRST_GROUP_ID + 1, 0)
            }
        );
        assert_eq!(rejection(unknown).code, RequestErrorCode::TrackDoesNotExist);
    }

    #[tokio::test]
    async fn fetch_is_served_from_the_cache_in_the_requested_group_order() {
        // Arrange
        let peers = origin_with_track("origin-fetch", RETENTION).await;
        let origin = &peers.origin;
        for (group, payload) in [
            (GroupBoundary::Next, b"a" as &[u8]),
            (GroupBoundary::Within, b"b"),
            (GroupBoundary::Next, b"c"),
            (GroupBoundary::Within, b"d"),
            (GroupBoundary::Within, b"e"),
            (GroupBoundary::Next, b"f"),
        ] {
            origin
                .write(NAMESPACE, TRACK, plain(group, payload))
                .await
                .unwrap();
        }
        origin
            .ensure_track(NAMESPACE, "empty", FIRST_GROUP_ID, RETENTION)
            .await;

        // Act
        let (ascending, ascending_objects) = fetch(
            &peers,
            TRACK,
            location(7, 1),
            location(8, 2),
            GroupOrder::Ascending,
        )
        .await
        .unwrap();
        let (descending, descending_objects) = fetch(
            &peers,
            TRACK,
            location(7, 0),
            location(10, 0),
            GroupOrder::Descending,
        )
        .await
        .unwrap();
        let unknown = fetch(
            &peers,
            "other",
            location(7, 0),
            location(8, 0),
            GroupOrder::Ascending,
        )
        .await;
        let empty = fetch(
            &peers,
            "empty",
            location(7, 0),
            location(8, 0),
            GroupOrder::Ascending,
        )
        .await;
        let invalid = fetch(
            &peers,
            TRACK,
            location(9, 1),
            location(10, 0),
            GroupOrder::Ascending,
        )
        .await;

        // Assert
        assert_eq!(ids(&ascending_objects), [(7, 1), (8, 0), (8, 1)]);
        assert_eq!(ascending.end_location, location(8, 2));
        assert_eq!(
            ids(&descending_objects),
            [(9, 0), (8, 0), (8, 1), (8, 2), (7, 0), (7, 1)]
        );
        assert_eq!(descending.end_location, location(9, 1));
        assert_eq!(rejection(unknown).code, RequestErrorCode::TrackDoesNotExist);
        assert_eq!(rejection(empty).code, RequestErrorCode::NoObjects);
        assert_eq!(rejection(invalid).code, RequestErrorCode::InvalidRange);
    }

    #[tokio::test]
    async fn a_track_released_after_a_subscribe_is_published_again() {
        // Arrange
        let peers = origin_with_track("origin-republish", RETENTION).await;
        let subscription = subscribe(&peers).await;
        unsubscribe(&peers, &subscription).await;

        // Act
        let accepted = publish(&peers).await;
        let written = peers
            .origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
            .await
            .unwrap();

        // Assert
        assert_eq!(accepted.track_name(), TRACK);
        assert!(matches!(written, WriteOutcome::Sent(_)));
    }

    #[tokio::test]
    async fn release_drops_the_writer_but_keeps_numbering_and_caching() {
        // Arrange
        let peers = origin_with_track("origin-release", RETENTION).await;
        let origin = &peers.origin;
        let subscription = subscribe(&peers).await;
        let live = origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
            .await
            .unwrap();

        // Act
        let released = unsubscribe(&peers, &subscription).await;
        let after_release = origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"b"))
            .await
            .unwrap();
        let (_, cached) = fetch(
            &peers,
            TRACK,
            location(FIRST_GROUP_ID + 1, 0),
            location(FIRST_GROUP_ID + 2, 0),
            GroupOrder::Ascending,
        )
        .await
        .unwrap();

        // Assert
        assert_eq!(live, sent(FIRST_GROUP_ID, 0, true));
        assert_eq!(released, Some((NAMESPACE.to_string(), TRACK.to_string())));
        assert_eq!(after_release, WriteOutcome::NotSent);
        assert_eq!(ids(&cached), [(FIRST_GROUP_ID + 1, 0)]);
    }

    #[tokio::test]
    async fn write_group_finishes_each_group_at_once_and_newest_only_keeps_the_last() {
        // Arrange
        let peers = origin_with_track("origin-write-group", Retention::NewestOnly).await;
        let origin = &peers.origin;
        let accepted = publish(&peers).await;

        // Act
        let first = origin
            .write_group(NAMESPACE, TRACK, Bytes::from_static(b"c1"))
            .await
            .unwrap();
        let second = origin
            .write_group(NAMESPACE, TRACK, Bytes::from_static(b"c2"))
            .await
            .unwrap();
        let mut reader = subscribed_track_reader(&peers.server, &accepted).await;
        let received_first = within(reader.next_object()).await.unwrap().unwrap();
        let received_second = within(reader.next_object()).await.unwrap().unwrap();
        let (handle, cached) = fetch(
            &peers,
            TRACK,
            location(0, 0),
            location(FIRST_GROUP_ID + 100, 0),
            GroupOrder::Ascending,
        )
        .await
        .unwrap();

        // Assert
        assert_eq!(first, sent(FIRST_GROUP_ID, 0, true));
        assert_eq!(second, sent(FIRST_GROUP_ID + 1, 0, true));
        assert_eq!(
            (received_first.group_id, received_first.payload),
            (FIRST_GROUP_ID, Bytes::from_static(b"c1"))
        );
        assert_eq!(
            (received_second.group_id, received_second.payload),
            (FIRST_GROUP_ID + 1, Bytes::from_static(b"c2"))
        );
        assert_eq!(ids(&cached), [(FIRST_GROUP_ID + 1, 0)]);
        assert_eq!(handle.end_location, location(FIRST_GROUP_ID + 1, 1));
    }

    #[tokio::test]
    async fn cancel_fetch_aborts_the_pending_response() {
        // Arrange
        let peers = origin_with_track("origin-fetch-cancel", RETENTION).await;
        let origin = &peers.origin;
        origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
            .await
            .unwrap();

        // Act
        let mut subscriber = peers.server.subscriber();
        let (result, ()) = within(async {
            tokio::join!(
                subscriber.fetch(
                    NAMESPACE.to_string(),
                    TRACK.to_string(),
                    location(FIRST_GROUP_ID, 0),
                    location(FIRST_GROUP_ID + 1, 0),
                    FetchOption::default()
                ),
                async {
                    let SessionEvent::Fetch(handler) = peers.client.receive_event().await.unwrap()
                    else {
                        panic!("expected FETCH from the relay");
                    };
                    let request_id = handler.request_id;
                    origin.serve_fetch(handler).await.unwrap();
                    origin.cancel_fetch(request_id).await;
                }
            )
        })
        .await;

        // Assert
        assert!(origin.state.lock().await.fetches.is_empty());
        assert_eq!(rejection(result).code, RequestErrorCode::NotSupported);
    }

    #[test]
    fn content_exists_reports_the_largest_sent_location() {
        // Act / Assert
        assert_eq!(content_exists(None), ContentExists::False);
        assert_eq!(
            content_exists(Some(location(3, 4))),
            ContentExists::True {
                location: location(3, 4)
            }
        );
    }
}
