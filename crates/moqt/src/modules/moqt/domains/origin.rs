use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use bytes::Bytes;
use futures::future::join_all;
use tokio::sync::Mutex;

use crate::modules::moqt::control_plane::error_codes::RequestErrorCode;

use crate::{
    Publisher, Session, TrackWriter, TransportSendError,
    fetch::{FetchCancelHandler, FetchHandler, FetchParams},
    modules::{executor, moqt::domains::session_context::SessionContext},
    object::{ContentExists, ExtensionHeaders, Location},
    publish::PublishOption,
    subscribe::{SubscribeHandler, TrackStatusHandler, UnsubscribeHandler},
};

mod fetch_serve_task;
mod object_cache;
mod object_numbering;

pub use object_cache::Retention;
pub use object_numbering::{GroupBoundary, Placement};

use fetch_serve_task::FetchServeTask;
use object_cache::{CachedObject, FetchRange, ObjectCache};
use object_numbering::ObjectNumbering;

/// An Origin never ends a subscription on a timer (draft-ietf-moq-transport-14
/// §9.8). Relays forward this value, and some subscribers (moq-ffi before
/// 0.4.5) reject a non-zero one.
const SUBSCRIBE_NEVER_EXPIRES: u64 = 0;
const FETCH_PUBLISHER_PRIORITY: u8 = 128;
const FETCH_SUBGROUP_ID: u64 = 0;

type TrackKey = (String, String);
/// The address of a session's `SessionContext`. An outlet or a FETCH task
/// holds the context while its key is in use, so the address is not reused.
type SessionKey = usize;
/// A request id is unique within its session only.
type RequestKey = (SessionKey, u64);

#[derive(Clone)]
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
    /// A peer stopped the subgroup stream and no other session sent the
    /// object; the object stays cached and the track resumes at the next
    /// group.
    Stopped(Placement),
    /// Dropped because no group is open for a `Within` object, or numbered and
    /// cached only: no session has a writer for the track, or every writer
    /// waits for the next group to start.
    NotSent,
}

/// Objects are numbered and cached once per track, from the first one handed
/// over, whether or not anything takes the track yet, so the cache and the
/// live streams agree on ids. Each subscription to the track (a SUBSCRIBE a
/// peer sent or a PUBLISH this endpoint sent) gets an outlet with its own
/// writer, and a write goes to every outlet; during a migration the old and
/// the new server each subscribe on their own. A writer created while a group
/// is open sends from the next group on. Content Exists reports what was
/// actually sent.
#[derive(Clone, Default)]
pub struct Origin {
    state: Arc<Mutex<OriginState>>,
}

#[derive(Default)]
struct OriginState {
    tracks: HashMap<TrackKey, OriginTrack>,
    subscriptions: HashMap<RequestKey, TrackKey>,
    fetches: HashMap<RequestKey, FetchServeTask>,
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
    /// A SUBSCRIBE the peer sent is being served.
    Subscribed,
    /// A PUBLISH this endpoint sent is being served.
    Published,
    /// A PUBLISH the peer unsubscribed from; kept so that `publish` does not
    /// send it again.
    Released,
}

struct OriginTrack {
    numbering: ObjectNumbering,
    cache: ObjectCache,
    largest_sent: Option<Location>,
    outlets: Vec<Outlet>,
}

/// One subscription to the track: a SUBSCRIBE served or a PUBLISH sent.
struct Outlet {
    request: RequestKey,
    publication: Publication,
    writer: Option<TrackWriter>,
}

impl OriginTrack {
    fn new(first_group_id: u64, retention: Retention) -> Self {
        Self {
            numbering: ObjectNumbering::new(first_group_id),
            cache: ObjectCache::new(retention),
            largest_sent: None,
            outlets: Vec::new(),
        }
    }

    fn outlet_mut(&mut self, request: RequestKey) -> Option<&mut Outlet> {
        self.outlets
            .iter_mut()
            .find(|outlet| outlet.request == request)
    }
}

impl Outlet {
    fn serving(&self) -> bool {
        self.publication != Publication::Released
    }
}

impl Origin {
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

    /// Sends PUBLISH on `session`. A no-op while the peer of `session` takes
    /// the track, and once that peer unsubscribed from a PUBLISH of it. A
    /// track released after a SUBSCRIBE is published again. A session that
    /// follows GOAWAY sends the track to every connection it is published on.
    pub async fn publish(
        &self,
        session: &Session,
        namespace: &str,
        track_name: &str,
    ) -> Result<()> {
        let key = track_key(namespace, track_name);
        let context = session.context();
        let mut state = self.state.lock().await;
        let track = state.track_mut(&key)?;
        let next_group_id = track.numbering.next_group_id();
        let key_of_session = session_key(context);
        if track
            .outlets
            .iter()
            .any(|outlet| outlet.request.0 == key_of_session)
        {
            return Ok(());
        }
        let publisher = session.publisher();
        let subscription = publisher
            .publish(
                namespace.to_string(),
                track_name.to_string(),
                PublishOption::default(),
            )
            .await
            .with_context(|| format!("publish {namespace}/{track_name}"))?;
        let request = (session_key(context), subscription.request_id());
        state.track_mut(&key)?.outlets.push(Outlet {
            request,
            publication: Publication::Published,
            writer: Some(TrackWriter::new(
                publisher.create_stream(&subscription),
                next_group_id,
            )),
        });
        state.subscriptions.insert(request, key);
        tracing::info!(
            namespace,
            track_name,
            track_alias = subscription.track_alias(),
            "track published"
        );
        Ok(())
    }

    /// Sends the object on every session that takes the track. A send error
    /// is returned only when no session sent the object and none stopped it.
    pub async fn write(
        &self,
        namespace: &str,
        track_name: &str,
        object: OutgoingObject,
    ) -> Result<WriteOutcome> {
        let key = track_key(namespace, track_name);
        let (placement, writers) = {
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
            (placement, take_writers(track))
        };
        let results = join_all(writers.into_iter().map(|(session, mut writer)| {
            let object = object.clone();
            async move {
                let result = send_placed(&mut writer, placement, object).await;
                (session, writer, result)
            }
        }))
        .await;
        self.settle(&key, placement, results).await
    }

    pub async fn write_group(
        &self,
        namespace: &str,
        track_name: &str,
        payload: Bytes,
    ) -> Result<WriteOutcome> {
        let key = track_key(namespace, track_name);
        let (placement, writers) = {
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
            (placement, take_writers(track))
        };
        let results = join_all(writers.into_iter().map(|(session, mut writer)| {
            let payload = payload.clone();
            async move {
                let result = writer
                    .write_group_at(placement.location.group_id, payload)
                    .await
                    .context("send group")
                    .map(|()| true);
                (session, writer, result)
            }
        }))
        .await;
        self.settle(&key, placement, results).await
    }

    /// Serves the SUBSCRIBE on the session it arrived on.
    pub async fn accept_subscribe(&self, handler: SubscribeHandler) -> Result<()> {
        let key = (handler.track_namespace.clone(), handler.track_name.clone());
        let content_exists = {
            let mut state = self.state.lock().await;
            content_exists(state.track_mut(&key)?.largest_sent)
        };
        let track_alias = handler
            .ok(SUBSCRIBE_NEVER_EXPIRES, content_exists)
            .await
            .context("accept SUBSCRIBE")?;
        let subscription = handler.into_subscription(track_alias);
        let context = &handler.session_context;
        let publisher = Publisher {
            session: context.clone(),
        };
        let mut state = self.state.lock().await;
        let track = state.track_mut(&key)?;
        let next_group_id = track.numbering.next_group_id();
        let request = (session_key(context), handler.request_id());
        track.outlets.push(Outlet {
            request,
            publication: Publication::Subscribed,
            writer: Some(TrackWriter::new(
                publisher.create_stream(&subscription),
                next_group_id,
            )),
        });
        state.subscriptions.insert(request, key);
        tracing::info!(
            namespace = %handler.track_namespace,
            track_name = %handler.track_name,
            track_alias,
            "SUBSCRIBE accepted"
        );
        Ok(())
    }

    /// Whether the peer of `session` still takes objects of any track: a
    /// SUBSCRIBE it sent or a PUBLISH it accepted that it has not
    /// unsubscribed from.
    pub async fn is_serving(&self, session: &Session) -> bool {
        let key = session_key(session.context());
        self.state
            .lock()
            .await
            .subscriptions
            .keys()
            .any(|(session, _)| *session == key)
    }

    /// Returns the released track's `(namespace, name)`; its numbering and
    /// cache are kept.
    pub async fn release(&self, handler: &UnsubscribeHandler) -> Option<(String, String)> {
        let request = (
            session_key(&handler.session_context),
            handler.subscribe_id(),
        );
        let mut state = self.state.lock().await;
        let key = state.subscriptions.remove(&request)?;
        if let Some(track) = state.tracks.get_mut(&key) {
            track.outlets.retain_mut(|outlet| {
                if outlet.request != request {
                    return true;
                }
                if outlet.publication != Publication::Published {
                    return false;
                }
                outlet.publication = Publication::Released;
                outlet.writer = None;
                true
            });
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
                let request_key = (session_key(&handler.session_context), handler.request_id);
                let task = FetchServeTask::run(
                    Arc::downgrade(&self.state),
                    request_key,
                    handler,
                    objects,
                    end_location,
                );
                state.fetches.insert(request_key, task);
                return Ok(());
            }
        };
        drop(state);
        handler.error(code, reason.to_string()).await?;
        Ok(())
    }

    pub async fn cancel_fetch(&self, handler: &FetchCancelHandler) {
        let request_key = (session_key(&handler.session_context), handler.request_id());
        if self
            .state
            .lock()
            .await
            .fetches
            .remove(&request_key)
            .is_some()
        {
            tracing::info!(
                request_id = handler.request_id(),
                "FETCH_CANCEL received; response aborted"
            );
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
        placement: Placement,
        results: Vec<(RequestKey, TrackWriter, Result<bool>)>,
    ) -> Result<WriteOutcome> {
        let mut state = self.state.lock().await;
        let track = state.track_mut(key)?;
        let (mut sent, mut stopped, mut failure) = (false, false, None);
        for (request, writer, result) in results {
            if let Some(outlet) = track.outlet_mut(request)
                && outlet.writer.is_none()
                && outlet.serving()
            {
                outlet.writer = Some(writer);
            }
            match result {
                Ok(true) => sent = true,
                Ok(false) => {}
                Err(error) if TransportSendError::is_stopped_by_peer(&error) => {
                    tracing::info!(
                        namespace = %key.0,
                        track_name = %key.1,
                        "subscriber stopped a subgroup; the track resumes at the next group"
                    );
                    stopped = true;
                }
                Err(error) => {
                    tracing::debug!(namespace = %key.0, track_name = %key.1, %error, "send failed on one session");
                    failure.get_or_insert(error);
                }
            }
        }
        if sent {
            track.largest_sent = Some(placement.location);
            return Ok(WriteOutcome::Sent(placement));
        }
        if stopped {
            return Ok(WriteOutcome::Stopped(placement));
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(WriteOutcome::NotSent),
        }
    }
}

fn take_writers(track: &mut OriginTrack) -> Vec<(RequestKey, TrackWriter)> {
    track
        .outlets
        .iter_mut()
        .filter_map(|outlet| outlet.writer.take().map(|writer| (outlet.request, writer)))
        .collect()
}

fn session_key(session: &Arc<SessionContext>) -> SessionKey {
    Arc::as_ptr(session) as SessionKey
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
        fetch::{FetchHandle, FetchOption},
        modules::test_support::{
            HANDSHAKE_TIMEOUT, accept_publish, read_fetch_objects, rejection,
            spawn_connected_dual_sessions, subscribed_track_reader,
        },
        object::{GroupOrder, Object},
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
        let origin = Origin::default();
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

    fn ids(objects: &[Object]) -> Vec<(u64, u64)> {
        objects
            .iter()
            .map(|object| (object.location.group_id, object.location.object_id))
            .collect()
    }

    async fn publish(peers: &Peers) -> Subscription {
        let (published, accepted) = within(async {
            tokio::join!(
                peers.origin.publish(&peers.client, NAMESPACE, TRACK),
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
                peers.origin.release(&handler).await
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
    ) -> Result<(FetchHandle, Vec<Object>)> {
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
    async fn accepted_subscribe_never_expires() {
        // Arrange
        let peers = origin_with_track("origin-subscribe-expires", RETENTION).await;

        // Act
        let subscription = subscribe(&peers).await;

        // Assert
        assert_eq!(subscription.expires(), Some(0));
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
    async fn a_group_written_while_another_is_in_flight_does_not_stall_the_track() {
        // Arrange
        let peers = origin_with_track("origin-write-group-in-flight", RETENTION).await;
        let origin = &peers.origin;
        let accepted = publish(&peers).await;
        let key = track_key(NAMESPACE, TRACK);
        origin
            .write_group(NAMESPACE, TRACK, Bytes::from_static(b"c0"))
            .await
            .unwrap();
        let in_flight = origin.state.lock().await.track_mut(&key).unwrap().outlets[0]
            .writer
            .take()
            .unwrap();

        // Act
        let overlapped = origin
            .write_group(NAMESPACE, TRACK, Bytes::from_static(b"c1"))
            .await
            .unwrap();
        origin.state.lock().await.track_mut(&key).unwrap().outlets[0].writer = Some(in_flight);
        let after = origin
            .write_group(NAMESPACE, TRACK, Bytes::from_static(b"c2"))
            .await
            .unwrap();
        let mut reader = subscribed_track_reader(&peers.server, &accepted).await;
        let received_first = within(reader.next_object()).await.unwrap().unwrap();
        let received_after = within(reader.next_object()).await.unwrap().unwrap();

        // Assert
        assert_eq!(overlapped, WriteOutcome::NotSent);
        assert_eq!(after, sent(FIRST_GROUP_ID + 2, 0, true));
        assert_eq!(received_first.group_id, FIRST_GROUP_ID);
        assert_eq!(
            (received_after.group_id, received_after.payload),
            (FIRST_GROUP_ID + 2, Bytes::from_static(b"c2"))
        );
        assert_eq!(
            received_after.extension_headers.prior_group_id_gap(),
            vec![1]
        );
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
                    let cancel = FetchCancelHandler::new(
                        peers.client.context().clone(),
                        &peers.client.context().connection(),
                        crate::modules::moqt::control_plane::control_messages::messages::fetch_cancel::FetchCancel {
                            request_id: handler.request_id,
                        },
                    );
                    origin.serve_fetch(handler).await.unwrap();
                    origin.cancel_fetch(&cancel).await;
                }
            )
        })
        .await;

        // Assert
        assert!(origin.state.lock().await.fetches.is_empty());
        assert_eq!(rejection(result).code, RequestErrorCode::NotSupported);
    }

    #[tokio::test]
    async fn a_track_taken_on_two_sessions_is_written_to_both_until_one_releases_it() {
        // Arrange
        let first = origin_with_track("origin-two-sessions-a", RETENTION).await;
        let (client, server) = spawn_connected_dual_sessions("origin-two-sessions-b").await;
        let second = Peers {
            origin: first.origin.clone(),
            client: Arc::new(client),
            server,
        };
        let accepted_first = publish(&first).await;
        let accepted_second = publish(&second).await;
        let origin = &first.origin;

        // Act
        let both = origin
            .write_group(NAMESPACE, TRACK, Bytes::from_static(b"g0"))
            .await
            .unwrap();
        let first_object = next_object(&first, &accepted_first).await;
        let second_object = next_object(&second, &accepted_second).await;
        unsubscribe(&first, &accepted_first).await;
        let one = origin
            .write_group(NAMESPACE, TRACK, Bytes::from_static(b"g1"))
            .await
            .unwrap();

        // Assert
        assert_eq!(both, sent(FIRST_GROUP_ID, 0, true));
        assert_eq!(first_object.payload, Bytes::from_static(b"g0"));
        assert_eq!(second_object.payload, Bytes::from_static(b"g0"));
        assert!(!origin.is_serving(&first.client).await);
        assert!(origin.is_serving(&second.client).await);
        assert_eq!(one, sent(FIRST_GROUP_ID + 1, 0, true));
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
