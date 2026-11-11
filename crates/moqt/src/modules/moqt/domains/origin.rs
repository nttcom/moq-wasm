use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use bytes::Bytes;
use futures::future::join_all;
use tokio::sync::{Mutex, Notify, mpsc};

use crate::modules::moqt::control_plane::error_codes::{PublishDoneCode, RequestErrorCode};

use crate::{
    Publisher, Session, SessionEvent, Subscription, TrackProperties, TrackWriter,
    TransportSendError,
    fetch::{
        FetchCancelHandler, FetchHandler, FetchParams, JoinPoint, JoiningFetchError, JoiningStart,
        resolve_joining_fetch,
    },
    modules::{executor, moqt::domains::session_context::SessionContext},
    object::{ContentExists, ExtensionHeaders, Location},
    publish::PublishOption,
    subscribe::{
        FilterType, SubscribeHandler, SubscribeUpdateHandler, TrackStatusHandler,
        UnsubscribeHandler,
    },
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
    /// cached only: no session has a writer for the track, or every writer starts
    /// after the object or waits for the next group after a stopped stream.
    NotSent,
}

/// Objects are numbered and cached once per track, from the first one handed
/// over, whether or not anything takes the track yet, so the cache and the
/// live streams agree on ids. Each subscription to the track (a SUBSCRIBE a
/// peer sent or a PUBLISH this endpoint sent) gets an outlet with its own
/// writer, and a write goes to every outlet; during a migration the old and
/// the new server each subscribe on their own. A PUBLISH sends from the next
/// group; a SUBSCRIBE sends from its filter's Start Location
/// (draft-ietf-moq-transport-14 §9.7), so a Largest Object filter takes the
/// rest of the open group. Content Exists reports the largest cached object,
/// the last one FETCH can serve.
#[derive(Clone, Default)]
pub struct Origin {
    state: Arc<Mutex<OriginState>>,
    /// Makes `publish` calls run one at a time while `state` is not held across
    /// the PUBLISH_OK wait, so two calls cannot both send PUBLISH. `release`
    /// waits on it for a request it does not know yet. Taken before `state`,
    /// never while holding it.
    publishing: Arc<Mutex<()>>,
}

#[derive(Default)]
struct OriginState {
    tracks: HashMap<TrackKey, OriginTrack>,
    subscriptions: HashMap<RequestKey, OriginSubscription>,
    fetches: HashMap<RequestKey, FetchServeTask>,
}

struct OriginSubscription {
    track: TrackKey,
    /// `None` for a PUBLISH this endpoint sent, which no Joining Fetch names.
    join_point: Option<JoinPoint>,
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
    /// Marked by `end_track` while a write held the writer; the write ends the
    /// outlet when it returns the writer.
    Ending,
}

struct OriginTrack {
    numbering: ObjectNumbering,
    cache: ObjectCache,
    outlets: Vec<Outlet>,
    /// Set for a track with dynamic groups (draft-18 §12.6); notified when a
    /// subscriber asks for a new group.
    new_group_requested: Option<Arc<Notify>>,
    /// Set for a track published on demand.
    demand: Option<Demand>,
}

/// Whether a subscription to a track published on demand is in Forward State
/// 1, as `Origin::publish_on_demand` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackDemand {
    pub track_name: String,
    pub wanted: bool,
}

struct Demand {
    /// The last one sent.
    reported: TrackDemand,
    demand_sender: mpsc::UnboundedSender<TrackDemand>,
}

/// One subscription to the track: a SUBSCRIBE served or a PUBLISH sent.
struct Outlet {
    request: RequestKey,
    publication: Publication,
    /// The Start Location (draft-14 §9.7); objects before it are not sent.
    start: Location,
    /// The Forward State (draft-14 §5.1, draft-18 §5.1); no object is sent
    /// while it is 0.
    forward: bool,
    writer: Option<TrackWriter>,
    publisher: Publisher,
    subscription: Subscription,
}

impl OriginTrack {
    fn new(first_group_id: u64, retention: Retention) -> Self {
        Self {
            numbering: ObjectNumbering::new(first_group_id),
            cache: ObjectCache::new(retention),
            outlets: Vec::new(),
            new_group_requested: None,
            demand: None,
        }
    }

    fn wanted(&self) -> bool {
        self.outlets
            .iter()
            .any(|outlet| outlet.serving() && outlet.forward)
    }

    /// Call after the outlets or their Forward State changed.
    fn refresh_demand(&mut self) {
        let wanted = self.wanted();
        let Some(demand) = &mut self.demand else {
            return;
        };
        if demand.reported.wanted != wanted {
            demand.reported.wanted = wanted;
            let _ = demand.demand_sender.send(demand.reported.clone());
        }
    }

    /// draft-18 §10.2.13: a request of 0, or one beyond the current group,
    /// asks for a new group; any other is for a group that already began.
    fn request_new_group(&self, new_group_request: u64) {
        let Some(requested) = &self.new_group_requested else {
            return;
        };
        let current_group = self.numbering.largest().map(|largest| largest.group_id);
        if new_group_request == 0 || current_group.is_none_or(|group| new_group_request > group) {
            requested.notify_one();
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

    /// Finishes the open group before PUBLISH_DONE, which draft-14 §9.12
    /// forbids until every stream of the subscription is closed.
    async fn end(self, writer: TrackWriter) -> Result<()> {
        let opened_streams = writer.opened_streams();
        if let Err(error) = writer.finish().await {
            tracing::debug!(?error, "open group could not be finished");
        }
        self.publisher
            .publish_done(
                &self.subscription,
                PublishDoneCode::TrackEnded,
                opened_streams,
                "track ended".to_string(),
            )
            .await
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

    /// Gives the track dynamic groups (draft-18 §12.6): its PUBLISH and
    /// SUBSCRIBE_OK say so, and `requested` is notified when a subscriber
    /// asks for a new group (NEW_GROUP_REQUEST, §10.2.13) that has not begun
    /// yet. The caller then starts one with the next object it writes. Call
    /// this before `publish` and before the track is subscribed to; fails
    /// for a track that is not registered.
    pub async fn accept_new_group_requests(
        &self,
        namespace: &str,
        track_name: &str,
        requested: Arc<Notify>,
    ) -> Result<()> {
        let key = track_key(namespace, track_name);
        self.state.lock().await.track_mut(&key)?.new_group_requested = Some(requested);
        Ok(())
    }

    /// Marks the track as published on demand: `publish` then asks for
    /// Forward State 0, so the peer has the track sent only while it wants
    /// the objects (draft-18 §10.10). `demand_sender` is sent whether a
    /// subscription to the track is in Forward State 1, now and at every
    /// change; while none is, no object written is sent, so the caller need
    /// not produce any. Call this before `publish`; fails for a track that is
    /// not registered.
    pub async fn publish_on_demand(
        &self,
        namespace: &str,
        track_name: &str,
        demand_sender: mpsc::UnboundedSender<TrackDemand>,
    ) -> Result<()> {
        let key = track_key(namespace, track_name);
        let mut state = self.state.lock().await;
        let track = state.track_mut(&key)?;
        let reported = TrackDemand {
            track_name: track_name.to_string(),
            wanted: track.wanted(),
        };
        let _ = demand_sender.send(reported.clone());
        track.demand = Some(Demand {
            reported,
            demand_sender,
        });
        Ok(())
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
        let key_of_session = session_key(context);
        let _publishing = self.publishing.lock().await;
        let option = {
            let mut state = self.state.lock().await;
            let track = state.track_mut(&key)?;
            if track
                .outlets
                .iter()
                .any(|outlet| outlet.request.0 == key_of_session)
            {
                return Ok(());
            }
            let mut track_properties = TrackProperties::default();
            track_properties.set_dynamic_groups(track.new_group_requested.is_some());
            PublishOption {
                forward: track.demand.is_none(),
                track_properties,
                ..PublishOption::default()
            }
        };
        let publisher = session.publisher();
        let subscription = publisher
            .publish(namespace.to_string(), track_name.to_string(), option)
            .await
            .with_context(|| format!("publish {namespace}/{track_name}"))?;
        let request = (key_of_session, subscription.request_id());
        let track_alias = subscription.track_alias();
        let forward = match &subscription {
            Subscription::PublisherInitiated(accepted) => accepted.forward,
            Subscription::SubscriberInitiated(_) => {
                unreachable!("a PUBLISH_OK gives a publisher-initiated subscription")
            }
        };
        let mut state = self.state.lock().await;
        let track = state.track_mut(&key)?;
        let next_group_id = track.numbering.next_group_id();
        let start = FilterType::NextGroupStart.start_location(track.numbering.largest());
        track.outlets.push(Outlet {
            request,
            publication: Publication::Published,
            start,
            forward,
            writer: Some(TrackWriter::new(
                publisher.create_stream(&subscription),
                next_group_id,
            )),
            publisher,
            subscription,
        });
        track.refresh_demand();
        state.subscriptions.insert(
            request,
            OriginSubscription {
                track: key,
                join_point: None,
            },
        );
        tracing::debug!(
            track_namespace = namespace,
            track_name,
            track_alias,
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
        let results = join_all(writers.into_iter().map(|(session, start, mut writer)| {
            let object = object.clone();
            async move {
                let result = send_placed(&mut writer, start, placement, object).await;
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
        let results = join_all(writers.into_iter().map(|(session, start, mut writer)| {
            let payload = payload.clone();
            async move {
                let result = if placement.location < start {
                    Ok(false)
                } else {
                    writer
                        .write_group_at(placement.location.group_id, payload)
                        .await
                        .context("send group")
                        .map(|()| true)
                };
                (session, writer, result)
            }
        }))
        .await;
        self.settle(&key, placement, results).await
    }

    /// Answers a SUBSCRIBE to a track registered with `ensure_track`, the
    /// UNSUBSCRIBE of a subscription it serves, FETCH, FETCH_CANCEL and
    /// TRACK_STATUS, logging a failed answer. Returns every other event for the
    /// caller to answer, including a SUBSCRIBE to an unregistered track and an
    /// UNSUBSCRIBE of a request it does not serve. A SUBSCRIBE_UPDATE is
    /// returned too, after its Forward State and NEW_GROUP_REQUEST were
    /// applied to the subscription it updates.
    pub async fn handle(&self, event: SessionEvent) -> Option<SessionEvent> {
        match event {
            SessionEvent::Subscribe(handler) => {
                let key = (handler.track_namespace.clone(), handler.track_name.clone());
                if !self.state.lock().await.tracks.contains_key(&key) {
                    return Some(SessionEvent::Subscribe(handler));
                }
                if let Err(err) = self.accept_subscribe(handler).await {
                    tracing::warn!(track_namespace = %key.0, track_name = %key.1, error = ?err, "failed to accept SUBSCRIBE");
                }
            }
            SessionEvent::Unsubscribe(handler) => {
                if self.release(&handler).await.is_none() {
                    return Some(SessionEvent::Unsubscribe(handler));
                }
            }
            SessionEvent::Fetch(handler) => {
                let request_id = handler.request_id;
                if let Err(err) = self.serve_fetch(handler).await {
                    tracing::warn!(request_id, error = ?err, "failed to answer FETCH");
                }
            }
            SessionEvent::FetchCancel(handler) => self.cancel_fetch(&handler).await,
            SessionEvent::SubscribeUpdate(handler) => {
                self.apply_update(&handler).await;
                return Some(SessionEvent::SubscribeUpdate(handler));
            }
            SessionEvent::TrackStatus(handler) => {
                let request_id = handler.request_id();
                if let Err(err) = self.answer_track_status(handler).await {
                    tracing::warn!(request_id, error = ?err, "failed to answer TRACK_STATUS");
                }
            }
            event => return Some(event),
        }
        None
    }

    async fn apply_update(&self, handler: &SubscribeUpdateHandler) {
        let request = (
            session_key(&handler.session_context),
            handler.subscription_request_id(),
        );
        let mut state = self.state.lock().await;
        let Some(key) = state
            .subscriptions
            .get(&request)
            .map(|subscription| subscription.track.clone())
        else {
            return;
        };
        let Some(track) = state.tracks.get_mut(&key) else {
            return;
        };
        if let Some(outlet) = track.outlet_mut(request) {
            outlet.forward = handler.forward();
        }
        track.refresh_demand();
        if let Some(new_group_request) = handler.new_group_request() {
            track.request_new_group(new_group_request);
        }
    }

    /// Serves the SUBSCRIBE on the session it arrived on.
    pub async fn accept_subscribe(&self, mut handler: SubscribeHandler) -> Result<()> {
        let key = (handler.track_namespace.clone(), handler.track_name.clone());
        // Hold the lock until the outlet is registered: the peer can act on
        // SUBSCRIBE_OK before then, and an `end_track` or `write` in that gap
        // would miss this subscription.
        let mut state = self.state.lock().await;
        let track = state.track_mut(&key)?;
        let largest = track.numbering.largest();
        handler
            .track_properties
            .set_dynamic_groups(track.new_group_requested.is_some());
        if let Some(new_group_request) = handler.new_group_request {
            track.request_new_group(new_group_request);
        }
        let track_alias = handler
            .ok(SUBSCRIBE_NEVER_EXPIRES, content_exists(largest))
            .await
            .context("accept SUBSCRIBE")?;
        let subscription = handler.into_subscription(track_alias);
        let context = &handler.session_context;
        let publisher = Publisher {
            session: context.clone(),
        };
        let track = state.track_mut(&key)?;
        let next_group_id = track.numbering.next_group_id();
        let start = handler
            .filter_type
            .start_location(track.numbering.largest());
        let request = (session_key(context), handler.request_id());
        track.outlets.push(Outlet {
            request,
            publication: Publication::Subscribed,
            start,
            forward: handler.forward,
            writer: Some(TrackWriter::new(
                publisher.create_stream(&subscription),
                next_group_id,
            )),
            publisher,
            subscription,
        });
        track.refresh_demand();
        state.subscriptions.insert(
            request,
            OriginSubscription {
                track: key,
                join_point: Some(JoinPoint::new(&handler.filter_type, largest)),
            },
        );
        tracing::info!(
            track_namespace = %handler.track_namespace,
            track_name = %handler.track_name,
            track_alias,
            "SUBSCRIBE accepted"
        );
        Ok(())
    }

    /// Whether the peer of `session` still takes objects of any track: a
    /// SUBSCRIBE it sent or a PUBLISH it accepted that it has not
    /// unsubscribed from.
    #[cfg(test)]
    async fn is_serving(&self, session: &Session) -> bool {
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
        if !state.subscriptions.contains_key(&request) {
            // The peer can UNSUBSCRIBE right after its PUBLISH_OK, before
            // `publish` registers the request; wait for that `publish` to finish.
            drop(state);
            let _publishing = self.publishing.lock().await;
            state = self.state.lock().await;
        }
        let key = state.subscriptions.remove(&request)?.track;
        let mut released = None;
        if let Some(track) = state.tracks.get_mut(&key) {
            track.outlets.retain_mut(|outlet| {
                if outlet.request != request {
                    return true;
                }
                released = outlet.writer.take();
                if outlet.publication != Publication::Published {
                    return false;
                }
                outlet.publication = Publication::Released;
                true
            });
            track.refresh_demand();
        }
        drop(state);
        if let Some(writer) = released {
            writer.cancel().await;
        }
        Some(key)
    }

    /// Ends every subscription to the track with PUBLISH_DONE (TRACK_ENDED,
    /// draft-ietf-moq-transport-14 §9.12). Does nothing for a track that is
    /// not registered. The track keeps its numbering and cache, so `publish`
    /// or a SUBSCRIBE takes it up again at the next group.
    pub async fn end_track(&self, namespace: &str, track_name: &str) -> Result<()> {
        let key = track_key(namespace, track_name);
        let ended = {
            let mut state = self.state.lock().await;
            let Some(track) = state.tracks.get_mut(&key) else {
                return Ok(());
            };
            let mut ended = Vec::new();
            let mut requests = Vec::new();
            for mut outlet in std::mem::take(&mut track.outlets) {
                requests.push(outlet.request);
                match outlet.writer.take() {
                    Some(writer) => ended.push((outlet, writer)),
                    None if outlet.serving() => {
                        outlet.publication = Publication::Ending;
                        track.outlets.push(outlet);
                    }
                    None => {}
                }
            }
            track.refresh_demand();
            for request in requests {
                state.subscriptions.remove(&request);
            }
            ended
        };
        end_outlets(ended).await
    }

    /// Serves a Standalone or Joining Fetch from the cache. A Joining Fetch
    /// names a SUBSCRIBE this origin accepted on the same session (draft-14
    /// §9.16.2).
    pub async fn serve_fetch(&self, handler: FetchHandler) -> Result<()> {
        let mut state = self.state.lock().await;
        let (key, start_location, end_location) = match fetch_target(&state, &handler) {
            Ok(target) => target,
            Err(error) => {
                drop(state);
                tracing::warn!(request_id = handler.request_id, %error, "Joining FETCH rejected");
                match error.request_error_code() {
                    Some(code) => handler.error(code, error.to_string()).await?,
                    None => handler.close_for_protocol_violation(&error.to_string()),
                }
                return Ok(());
            }
        };
        let range = state
            .tracks
            .get(&key)
            .map(|track| track.cache.resolve(start_location, end_location));
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
                    track_namespace = %key.0,
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
        let largest = self
            .state
            .lock()
            .await
            .tracks
            .get(&key)
            .map(|track| track.numbering.largest());
        match largest {
            // Its SUBSCRIBE_OK carries no Track Properties either.
            Some(largest) => {
                handler
                    .ok(0, content_exists(largest), TrackProperties::default())
                    .await?
            }
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
        // Writers whose outlet was released or replaced during the write; reset,
        // not dropped (a drop FINs).
        let mut orphaned = Vec::new();
        let mut ended = Vec::new();
        for (request, writer, result) in results {
            let ending = track.outlets.iter().position(|outlet| {
                outlet.request == request && outlet.publication == Publication::Ending
            });
            match (ending, track.outlet_mut(request)) {
                (Some(index), _) => ended.push((track.outlets.remove(index), writer)),
                (None, Some(outlet)) if outlet.writer.is_none() && outlet.serving() => {
                    outlet.writer = Some(writer)
                }
                _ => orphaned.push(writer),
            }
            match result {
                Ok(true) => sent = true,
                Ok(false) => {}
                Err(error) if TransportSendError::is_stopped_by_peer(&error) => {
                    tracing::info!(
                        track_namespace = %key.0,
                        track_name = %key.1,
                        "subscriber stopped a subgroup; the track resumes at the next group"
                    );
                    stopped = true;
                }
                Err(error) => {
                    tracing::debug!(track_namespace = %key.0, track_name = %key.1, %error, "send failed on one session");
                    failure.get_or_insert(error);
                }
            }
        }
        track.refresh_demand();
        drop(state);
        for writer in orphaned {
            writer.cancel().await;
        }
        if let Err(error) = end_outlets(ended).await {
            tracing::debug!(%error, "PUBLISH_DONE could not be sent");
        }
        if sent {
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

/// The track and the `[start, end)` range a FETCH asks for, in the Standalone
/// Fetch encoding.
fn fetch_target(
    state: &OriginState,
    handler: &FetchHandler,
) -> Result<(TrackKey, Location, Location), JoiningFetchError> {
    let (joining_request_id, joining_start) = match &handler.fetch.fetch_params {
        FetchParams::Standalone {
            track_namespace,
            track_name,
            start_location,
            end_location,
        } => {
            return Ok((
                (track_namespace.join("/"), track_name.clone()),
                *start_location,
                *end_location,
            ));
        }
        FetchParams::RelativeJoining {
            joining_request_id,
            joining_start,
        } => (*joining_request_id, JoiningStart::Relative(*joining_start)),
        FetchParams::AbsoluteJoining {
            joining_request_id,
            joining_start,
        } => (*joining_request_id, JoiningStart::Absolute(*joining_start)),
    };
    let request = (
        session_key(&handler.session_context),
        handler.logical_request_id(joining_request_id),
    );
    let (track, join_point) = state
        .subscriptions
        .get(&request)
        .and_then(|subscription| Some((&subscription.track, subscription.join_point?)))
        .ok_or(JoiningFetchError::UnknownSubscription)?;
    let (start, end) = resolve_joining_fetch(joining_start, join_point)?;
    Ok((track.clone(), start, end))
}

/// The writers of the subscriptions an object is sent on: those in Forward
/// State 1. One in Forward State 0 leaves its open group, which the object
/// is omitted from, and takes the group up again once it is back in 1.
fn take_writers(track: &mut OriginTrack) -> Vec<(RequestKey, Location, TrackWriter)> {
    track
        .outlets
        .iter_mut()
        .filter_map(|outlet| {
            if !outlet.forward {
                if let Some(writer) = &mut outlet.writer {
                    writer.abandon_group();
                }
                return None;
            }
            let writer = outlet.writer.take()?;
            Some((outlet.request, outlet.start, writer))
        })
        .collect()
}

/// Returns the first failure after trying every outlet.
async fn end_outlets(outlets: Vec<(Outlet, TrackWriter)>) -> Result<()> {
    let results = join_all(
        outlets
            .into_iter()
            .map(|(outlet, writer)| outlet.end(writer)),
    )
    .await;
    results.into_iter().collect()
}

fn session_key(session: &Arc<SessionContext>) -> SessionKey {
    Arc::as_ptr(session) as SessionKey
}

fn track_key(namespace: &str, track_name: &str) -> TrackKey {
    (namespace.to_string(), track_name.to_string())
}

fn content_exists(largest: Option<Location>) -> ContentExists {
    match largest {
        Some(location) => ContentExists::True { location },
        None => ContentExists::False,
    }
}

/// A writer that has sent nothing joins the group the object is in, and so
/// does one that left the group in Forward State 0; one whose group a peer
/// stopped or the transport reset resumes at the next group.
async fn send_placed(
    writer: &mut TrackWriter,
    start: Location,
    placement: Placement,
    object: OutgoingObject,
) -> Result<bool> {
    let Location {
        group_id,
        object_id,
    } = placement.location;
    if placement.location < start {
        return Ok(false);
    }
    if placement.starts_group {
        writer
            .start_group_at(group_id)
            .await
            .context("start group")?;
    } else if writer.abandoned_group_id() == Some(group_id) {
        writer
            .rejoin_group_at(object_id)
            .await
            .context("rejoin group")?;
    } else if writer.current_group_id() != Some(group_id) {
        if writer.groups() > 0 {
            return Ok(false);
        }
        writer
            .join_group_at(group_id, object_id)
            .await
            .context("join group")?;
    }
    writer
        .write_with_extension_headers(object.payload, object.extension_headers)
        .await
        .context("send subgroup object")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use futures::FutureExt;

    use super::*;
    use crate::modules::test_support::location;
    use crate::{
        RequestFailure, SessionEvent, Subscription, TrackObject, Version,
        fetch::{FetchHandle, FetchOption},
        modules::test_support::{
            DRAFTS, accept_publish, connected_sessions, data_receiver, next_subgroup,
            read_fetch_objects, rejection, subscribed_track_reader, within,
        },
        object::{DataReceiver, GroupOrder, Object, ObjectStatus, StreamDataReceiver, Subgroup},
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

    async fn origin_with_track(name: &str, retention: Retention, version: Version) -> Peers {
        let (client, server) = connected_sessions(name, version).await;
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

    fn subscribe_option(filter_type: FilterType) -> SubscribeOption {
        SubscribeOption {
            filter_type,
            ..SubscribeOption::default()
        }
    }

    async fn subscribe(peers: &Peers, filter_type: FilterType) -> Subscription {
        let option = subscribe_option(filter_type);
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

    /// Runs `request` while the origin serves the FETCH it sends.
    async fn served<T>(peers: &Peers, request: impl Future<Output = T>) -> T {
        let (result, ()) = within(async {
            tokio::join!(request, async {
                let SessionEvent::Fetch(handler) = peers.client.receive_event().await.unwrap()
                else {
                    panic!("expected FETCH from the relay");
                };
                peers.origin.serve_fetch(handler).await.unwrap();
            })
        })
        .await;
        result
    }

    async fn read_fetch(
        subscriber: &mut crate::Subscriber,
        handle: FetchHandle,
    ) -> Result<(FetchHandle, Vec<Object>)> {
        let mut receiver = subscriber.accept_fetch_receiver(&handle).await?;
        let objects = within(read_fetch_objects(&mut receiver)).await;
        Ok((handle, objects))
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
        let request = subscriber.fetch(
            NAMESPACE.to_string(),
            track_name.to_string(),
            start,
            end,
            option,
        );
        let handle = served(peers, request).await?;
        read_fetch(&mut subscriber, handle).await
    }

    async fn joining_fetch(
        peers: &Peers,
        subscription: &Subscription,
        joining_start: u64,
    ) -> Result<(FetchHandle, Vec<Object>)> {
        let mut subscriber = peers.server.subscriber();
        let request =
            subscriber.fetch_relative_joining(subscription, joining_start, FetchOption::default());
        let handle = served(peers, request).await?;
        read_fetch(&mut subscriber, handle).await
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

    async fn end_track(peers: &Peers) -> crate::publish::PublishDoneHandler {
        let (ended, done) = within(async {
            tokio::join!(peers.origin.end_track(NAMESPACE, TRACK), async {
                let SessionEvent::PublishDone(done) = peers.server.receive_event().await.unwrap()
                else {
                    panic!("expected PUBLISH_DONE");
                };
                done
            })
        })
        .await;
        ended.unwrap();
        done
    }

    async fn next_object(peers: &Peers, subscription: &Subscription) -> TrackObject {
        let mut reader = subscribed_track_reader(&peers.server, subscription).await;
        within(reader.next_object()).await.unwrap().unwrap()
    }

    /// Publishes the track to a peer that accepts it in the Forward State the
    /// PUBLISH asks for.
    async fn publish_accepted_as_asked(peers: &Peers) -> Subscription {
        let (published, accepted) = within(async {
            tokio::join!(
                peers.origin.publish(&peers.client, NAMESPACE, TRACK),
                async {
                    let SessionEvent::Publish(handler) =
                        peers.server.receive_event().await.unwrap()
                    else {
                        panic!("expected PUBLISH from the client");
                    };
                    let accepted = handler
                        .ok(128, FilterType::LargestObject, handler.forward)
                        .await
                        .unwrap();
                    handler.accept_data_receiver().await;
                    accepted
                }
            )
        })
        .await;
        published.unwrap();
        accepted
    }

    /// The peer sets the Forward State of `subscription` and the origin
    /// applies the update.
    async fn update_forward(peers: &Peers, subscription: &Subscription, forward: bool) {
        let subscriber = peers.server.subscriber();
        let (updated, ()) = within(async {
            tokio::join!(subscriber.update_forward(subscription, forward), async {
                let event = peers.client.receive_event().await.unwrap();
                assert!(matches!(event, SessionEvent::SubscribeUpdate(_)));
                peers.origin.handle(event).await;
            })
        })
        .await;
        updated.unwrap();
    }

    #[tokio::test]
    async fn a_track_published_on_demand_is_sent_only_while_a_subscription_forwards() {
        for version in DRAFTS {
            // Arrange
            let peers = origin_with_track("origin-on-demand", RETENTION, version).await;
            let origin = &peers.origin;
            let (demand_sender, mut demand_receiver) = mpsc::unbounded_channel();
            origin
                .publish_on_demand(NAMESPACE, TRACK, demand_sender)
                .await
                .unwrap();
            let accepted = publish_accepted_as_asked(&peers).await;

            // Act
            let before_update = origin
                .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
                .await
                .unwrap();
            update_forward(&peers, &accepted, true).await;
            let after_update = origin
                .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"b"))
                .await
                .unwrap();
            let received = next_object(&peers, &accepted).await;

            // Assert
            let demand = |wanted| TrackDemand {
                track_name: TRACK.to_string(),
                wanted,
            };
            assert_eq!(demand_receiver.try_recv(), Ok(demand(false)));
            assert_eq!(demand_receiver.try_recv(), Ok(demand(true)));
            assert_eq!(before_update, WriteOutcome::NotSent);
            assert_eq!(after_update, sent(FIRST_GROUP_ID + 1, 0, true));
            assert_eq!(
                (received.group_id, received.object_id),
                (FIRST_GROUP_ID + 1, 0)
            );
        }
    }

    #[tokio::test]
    async fn a_subscription_back_in_forward_state_1_takes_its_group_up_past_what_it_missed() {
        for version in DRAFTS {
            // Arrange: the subscription is in the middle of a group
            let peers = origin_with_track("origin-forward-off", RETENTION, version).await;
            let origin = &peers.origin;
            let accepted = publish(&peers).await;
            let write = |group, payload| origin.write(NAMESPACE, TRACK, plain(group, payload));
            let first = write(GroupBoundary::Next, b"a").await.unwrap();

            // Act
            update_forward(&peers, &accepted, false).await;
            let in_forward_state_0 = write(GroupBoundary::Within, b"b").await.unwrap();
            update_forward(&peers, &accepted, true).await;
            let same_group = write(GroupBoundary::Within, b"c").await.unwrap();
            let next_group = write(GroupBoundary::Next, b"d").await.unwrap();

            // Assert
            assert_eq!(first, sent(FIRST_GROUP_ID, 0, true));
            assert_eq!(in_forward_state_0, WriteOutcome::NotSent);
            assert_eq!(same_group, sent(FIRST_GROUP_ID, 2, false));
            assert_eq!(next_group, sent(FIRST_GROUP_ID + 1, 0, true));
        }
    }

    #[tokio::test]
    async fn write_caches_before_publish_and_sends_from_the_next_group_after_it() {
        for version in DRAFTS {
            // Arrange
            let peers = origin_with_track("origin-publish", RETENTION, version).await;
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
    }

    #[tokio::test]
    async fn a_write_does_not_wait_for_an_unanswered_publish() {
        // Arrange
        let peers =
            origin_with_track("origin-publish-unanswered", RETENTION, Version::Draft14).await;
        let origin = &peers.origin;

        // Act
        let (published, (written, accepted)) = within(async {
            tokio::join!(origin.publish(&peers.client, NAMESPACE, TRACK), async {
                let SessionEvent::Publish(handler) = peers.server.receive_event().await.unwrap()
                else {
                    panic!("expected PUBLISH from the client");
                };
                let written = origin
                    .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
                    .await
                    .unwrap();
                let accepted = handler
                    .ok(128, FilterType::LargestObject, true)
                    .await
                    .unwrap();
                handler.accept_data_receiver().await;
                (written, accepted)
            })
        })
        .await;
        let next_group = origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"b"))
            .await
            .unwrap();
        let received = next_object(&peers, &accepted).await;

        // Assert: the write while PUBLISH_OK was pending is cached only; the track is sent from the next group
        published.unwrap();
        assert_eq!(written, WriteOutcome::NotSent);
        assert_eq!(next_group, sent(FIRST_GROUP_ID + 1, 0, true));
        assert_eq!(
            (received.group_id, received.payload),
            (FIRST_GROUP_ID + 1, Bytes::from_static(b"b"))
        );
    }

    #[tokio::test]
    async fn accepted_subscribe_never_expires() {
        for version in DRAFTS {
            // Arrange
            let peers = origin_with_track("origin-subscribe-expires", RETENTION, version).await;

            // Act
            let subscription = subscribe(&peers, FilterType::NextGroupStart).await;

            // Assert
            assert_eq!(subscription.expires(), Some(0));
        }
    }

    #[tokio::test]
    async fn accepted_subscribe_starts_at_the_next_group_and_reports_the_largest_cached_object() {
        for version in DRAFTS {
            // Arrange
            let peers = origin_with_track("origin-subscribe", RETENTION, version).await;
            let origin = &peers.origin;
            origin
                .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
                .await
                .unwrap();

            // Act
            let subscription = subscribe(&peers, FilterType::NextGroupStart).await;
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
            assert_eq!(
                subscription.content_exists(),
                ContentExists::True {
                    location: location(FIRST_GROUP_ID, 0)
                }
            );
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
    }

    #[tokio::test]
    async fn a_largest_object_subscribe_takes_the_rest_of_the_open_group() {
        for version in DRAFTS {
            // Arrange
            let peers = origin_with_track("origin-subscribe-largest", RETENTION, version).await;
            let origin = &peers.origin;
            origin
                .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
                .await
                .unwrap();

            // Act
            let subscription = subscribe(&peers, FilterType::LargestObject).await;
            let mid_group = origin
                .write(NAMESPACE, TRACK, plain(GroupBoundary::Within, b"b"))
                .await
                .unwrap();
            let received = next_object(&peers, &subscription).await;

            // Assert
            assert_eq!(mid_group, sent(FIRST_GROUP_ID, 1, false));
            assert_eq!(
                (received.group_id, received.object_id, received.payload),
                (FIRST_GROUP_ID, 1, Bytes::from_static(b"b"))
            );
        }
    }

    #[tokio::test]
    async fn an_absolute_start_subscribe_skips_what_precedes_its_start() {
        for version in DRAFTS {
            // Arrange
            let peers = origin_with_track("origin-subscribe-absolute", RETENTION, version).await;
            let origin = &peers.origin;
            let start = location(FIRST_GROUP_ID + 1, 1);

            // Act
            let subscription =
                subscribe(&peers, FilterType::AbsoluteStart { location: start }).await;
            let mut outcomes = Vec::new();
            for (group, payload) in [
                (GroupBoundary::Next, b"a" as &'static [u8]),
                (GroupBoundary::Next, b"b"),
                (GroupBoundary::Within, b"c"),
            ] {
                outcomes.push(
                    origin
                        .write(NAMESPACE, TRACK, plain(group, payload))
                        .await
                        .unwrap(),
                );
            }
            let received = next_object(&peers, &subscription).await;

            // Assert
            assert_eq!(
                outcomes,
                [
                    WriteOutcome::NotSent,
                    WriteOutcome::NotSent,
                    sent(FIRST_GROUP_ID + 1, 1, false)
                ]
            );
            assert_eq!(
                (received.group_id, received.object_id, received.payload),
                (FIRST_GROUP_ID + 1, 1, Bytes::from_static(b"c"))
            );
        }
    }

    /// Subscribes from the next group, writes its first object and receives
    /// the header of its stream.
    async fn open_group_stream(peers: &Peers) -> (Subscription, DataReceiver, StreamDataReceiver) {
        let subscription = subscribe(peers, FilterType::NextGroupStart).await;
        peers
            .origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
            .await
            .unwrap();
        let mut streams = data_receiver(&peers.server, &subscription).await;
        let mut subgroup = within(next_subgroup(&mut streams)).await;
        assert!(matches!(
            within(subgroup.receive()).await,
            Ok(Some(Subgroup::Header(_)))
        ));
        (subscription, streams, subgroup)
    }

    #[tokio::test]
    async fn an_unsubscribe_resets_the_open_group_instead_of_finishing_it() {
        for version in DRAFTS {
            // Arrange: the subscriber reads the first object of a group that stays open
            let peers = origin_with_track("origin-release-reset", RETENTION, version).await;
            let (subscription, _streams, mut subgroup) = open_group_stream(&peers).await;
            assert!(matches!(
                within(subgroup.receive()).await,
                Ok(Some(Subgroup::Object(_)))
            ));

            // Act
            unsubscribe(&peers, &subscription).await;

            // Assert: the stream is reset (receive errors), not finished
            assert!(within(subgroup.receive()).await.is_err());
        }
    }

    #[tokio::test]
    async fn end_track_finishes_the_open_group_and_counts_every_stream_in_publish_done() {
        // Arrange: two groups were sent, the second one is still open
        let peers = origin_with_track("origin-end-track", RETENTION, Version::Draft14).await;
        let subscription = subscribe(&peers, FilterType::NextGroupStart).await;
        let mut streams = None;
        let mut subgroups = Vec::new();
        for payload in [b"a" as &'static [u8], b"b"] {
            peers
                .origin
                .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, payload))
                .await
                .unwrap();
            if streams.is_none() {
                streams = Some(data_receiver(&peers.server, &subscription).await);
            }
            let mut subgroup = within(next_subgroup(streams.as_mut().unwrap())).await;
            assert!(matches!(
                within(subgroup.receive()).await,
                Ok(Some(Subgroup::Header(_)))
            ));
            assert!(matches!(
                within(subgroup.receive()).await,
                Ok(Some(Subgroup::Object(_)))
            ));
            subgroups.push(subgroup);
        }

        // Act
        let done = end_track(&peers).await;
        let end_of_group = within(subgroups[1].receive()).await;
        let fin = within(subgroups[1].receive()).await;

        // Assert: the open group ends with End of Group and a FIN, not a reset
        assert_eq!(done.request_id(), subscription.request_id());
        assert_eq!(done.status_code(), PublishDoneCode::TrackEnded);
        assert_eq!(done.stream_count(), 2);
        assert!(
            matches!(&end_of_group, Ok(Some(Subgroup::Object(object))) if object.status == ObjectStatus::EndOfGroup),
            "{end_of_group:?}"
        );
        assert!(matches!(fin, Ok(None)), "{fin:?}");
    }

    #[tokio::test]
    async fn an_ended_track_is_published_again_and_continues_its_group_ids() {
        // Arrange
        let peers =
            origin_with_track("origin-end-track-republish", RETENTION, Version::Draft14).await;
        publish(&peers).await;
        peers
            .origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
            .await
            .unwrap();
        let done = end_track(&peers).await;

        // Act
        let while_ended = peers
            .origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"b"))
            .await
            .unwrap();
        let accepted = publish(&peers).await;
        let after = peers
            .origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"c"))
            .await
            .unwrap();
        let received = next_object(&peers, &accepted).await;

        // Assert
        assert_eq!(done.stream_count(), 1);
        assert_eq!(while_ended, WriteOutcome::NotSent);
        assert_eq!(after, sent(FIRST_GROUP_ID + 2, 0, true));
        assert_eq!(
            (received.group_id, received.payload),
            (FIRST_GROUP_ID + 2, Bytes::from_static(b"c"))
        );
    }

    #[tokio::test]
    async fn a_subgroup_the_subscriber_stops_is_reported_and_the_track_resumes_at_the_next_group() {
        for version in DRAFTS {
            // Arrange: the subscriber has the header of an open group's stream
            let peers = origin_with_track("origin-stop-sending", RETENTION, version).await;
            let origin = &peers.origin;
            let (_subscription, mut streams, stopped) = open_group_stream(&peers).await;

            // Act: dropping the stream sends STOP_SENDING; writes go on until it arrives
            drop(stopped);
            let outcome = within(async {
                loop {
                    let outcome = origin
                        .write(NAMESPACE, TRACK, plain(GroupBoundary::Within, b"b"))
                        .await
                        .unwrap();
                    if !matches!(outcome, WriteOutcome::Sent(_)) {
                        return outcome;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
            let within_stopped = origin
                .write(NAMESPACE, TRACK, plain(GroupBoundary::Within, b"c"))
                .await
                .unwrap();
            let next_group = origin
                .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"d"))
                .await
                .unwrap();
            let mut resumed = within(next_subgroup(&mut streams)).await;
            let header = within(resumed.receive()).await;
            let object = within(resumed.receive()).await;

            // Assert: the stopped group gets nothing more; the next one is delivered
            assert!(matches!(outcome, WriteOutcome::Stopped(_)), "{outcome:?}");
            assert_eq!(within_stopped, WriteOutcome::NotSent);
            assert_eq!(next_group, sent(FIRST_GROUP_ID + 1, 0, true));
            assert!(matches!(header, Ok(Some(Subgroup::Header(_)))));
            let Ok(Some(Subgroup::Object(object))) = object else {
                panic!("expected an object of the next group, got {object:?}");
            };
            assert_eq!(object.location, location(FIRST_GROUP_ID + 1, 0));
            assert_eq!(object.payload, Bytes::from_static(b"d"));
        }
    }

    #[tokio::test]
    async fn fetch_is_served_from_the_cache_in_the_requested_group_order() {
        for version in DRAFTS {
            // Arrange
            let peers = origin_with_track("origin-fetch", RETENTION, version).await;
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
            let no_objects = match version {
                Version::Draft14 => RequestErrorCode::NoObjects,
                // draft-18 has no NO_OBJECTS; it answers INVALID_RANGE (§10.12.3)
                Version::Draft18 => RequestErrorCode::InvalidRange,
            };
            assert_eq!(rejection(empty).code, no_objects);
            assert_eq!(rejection(invalid).code, RequestErrorCode::InvalidRange);
        }
    }

    #[tokio::test]
    async fn a_track_released_after_a_subscribe_is_published_again() {
        for version in DRAFTS {
            // Arrange
            let peers = origin_with_track("origin-republish", RETENTION, version).await;
            let subscription = subscribe(&peers, FilterType::NextGroupStart).await;
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
    }

    #[tokio::test]
    async fn release_drops_the_writer_but_keeps_numbering_and_caching() {
        for version in DRAFTS {
            // Arrange
            let peers = origin_with_track("origin-release", RETENTION, version).await;
            let origin = &peers.origin;
            let subscription = subscribe(&peers, FilterType::NextGroupStart).await;
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
    }

    #[tokio::test]
    async fn write_group_finishes_each_group_at_once_and_newest_only_keeps_the_last() {
        for version in DRAFTS {
            // Arrange
            let peers =
                origin_with_track("origin-write-group", Retention::NewestOnly, version).await;
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
    }

    #[tokio::test]
    async fn a_group_written_while_another_is_in_flight_does_not_stall_the_track() {
        for version in DRAFTS {
            // Arrange
            let peers = origin_with_track("origin-write-group-in-flight", RETENTION, version).await;
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
    }

    #[tokio::test]
    async fn a_joining_fetch_ends_at_the_largest_object_its_subscribe_ok_reported() {
        // Arrange
        let peers = origin_with_track("origin-joining-fetch", RETENTION, Version::Draft14).await;
        let origin = &peers.origin;
        for group in [
            GroupBoundary::Next,
            GroupBoundary::Next,
            GroupBoundary::Within,
        ] {
            origin
                .write(NAMESPACE, TRACK, plain(group, b"a"))
                .await
                .unwrap();
        }
        let subscription = subscribe(&peers, FilterType::LargestObject).await;
        origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Within, b"live"))
            .await
            .unwrap();

        // Act
        let (current_group, current_group_objects) =
            joining_fetch(&peers, &subscription, 0).await.unwrap();
        let (previous_groups, previous_groups_objects) =
            joining_fetch(&peers, &subscription, 1).await.unwrap();

        // Assert
        assert_eq!(current_group.end_location, location(8, 2));
        assert_eq!(ids(&current_group_objects), vec![(8, 0), (8, 1)]);
        assert_eq!(previous_groups.end_location, location(8, 2));
        assert_eq!(ids(&previous_groups_objects), vec![(7, 0), (8, 0), (8, 1)]);
    }

    #[tokio::test]
    async fn a_joining_fetch_of_a_subscription_answered_without_content_is_an_invalid_range() {
        // Arrange
        let peers =
            origin_with_track("origin-joining-fetch-empty", RETENTION, Version::Draft14).await;
        let subscription = subscribe(&peers, FilterType::LargestObject).await;
        peers
            .origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
            .await
            .unwrap();

        // Act
        let result = joining_fetch(&peers, &subscription, 0).await;

        // Assert
        assert_eq!(rejection(result).code, RequestErrorCode::InvalidRange);
    }

    #[tokio::test]
    async fn a_joining_fetch_of_a_released_subscription_names_an_invalid_request_id() {
        // Arrange
        let peers =
            origin_with_track("origin-joining-fetch-released", RETENTION, Version::Draft14).await;
        peers
            .origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
            .await
            .unwrap();
        let subscription = subscribe(&peers, FilterType::LargestObject).await;
        unsubscribe(&peers, &subscription).await.unwrap();

        // Act
        let result = joining_fetch(&peers, &subscription, 0).await;

        // Assert
        assert_eq!(
            rejection(result).code,
            RequestErrorCode::InvalidJoiningRequestId
        );
    }

    #[tokio::test]
    async fn a_joining_fetch_of_a_next_group_start_subscription_closes_the_session() {
        // Arrange
        let peers = origin_with_track(
            "origin-joining-fetch-violation",
            RETENTION,
            Version::Draft14,
        )
        .await;
        peers
            .origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
            .await
            .unwrap();
        let subscription = subscribe(&peers, FilterType::NextGroupStart).await;

        // Act
        let mut subscriber = peers.server.subscriber();
        let (result, event) = within(async {
            tokio::join!(
                subscriber.fetch_relative_joining(&subscription, 0, FetchOption::default()),
                async {
                    let SessionEvent::Fetch(handler) = peers.client.receive_event().await.unwrap()
                    else {
                        panic!("expected FETCH from the relay");
                    };
                    peers.origin.serve_fetch(handler).await.unwrap();
                    peers.client.receive_event().await.unwrap()
                }
            )
        })
        .await;

        // Assert
        assert!(result.is_err());
        assert!(matches!(event, SessionEvent::ProtocolViolation()));
    }

    #[tokio::test]
    async fn cancel_fetch_aborts_the_pending_response() {
        for version in DRAFTS {
            // Arrange
            let peers = origin_with_track("origin-fetch-cancel", RETENTION, version).await;
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
    }

    #[tokio::test]
    async fn a_track_taken_on_two_sessions_is_written_to_both_until_one_releases_it() {
        for version in DRAFTS {
            // Arrange
            let first = origin_with_track("origin-two-sessions-a", RETENTION, version).await;
            let (client, server) = connected_sessions("origin-two-sessions-b", version).await;
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
    }

    #[test]
    fn content_exists_reports_the_largest_location() {
        // Act / Assert
        assert_eq!(content_exists(None), ContentExists::False);
        assert_eq!(
            content_exists(Some(location(3, 4))),
            ContentExists::True {
                location: location(3, 4)
            }
        );
    }

    #[tokio::test]
    async fn handle_serves_a_subscribe_to_a_track_it_has() {
        for version in DRAFTS {
            // Arrange
            let peers = origin_with_track("origin-handle-subscribe", RETENTION, version).await;
            let mut subscriber = peers.server.subscriber();

            // Act
            let (subscription, unhandled) = within(async {
                tokio::join!(
                    subscriber.subscribe(
                        NAMESPACE.to_string(),
                        TRACK.to_string(),
                        subscribe_option(FilterType::NextGroupStart)
                    ),
                    async {
                        let event = peers.client.receive_event().await.unwrap();
                        peers.origin.handle(event).await
                    }
                )
            })
            .await;

            // Assert
            assert!(unhandled.is_none());
            let subscription = subscription.unwrap();
            peers
                .origin
                .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
                .await
                .unwrap();
            assert_eq!(
                next_object(&peers, &subscription).await.payload,
                Bytes::from_static(b"a")
            );
        }
    }

    #[tokio::test]
    async fn handle_returns_a_subscribe_to_a_track_it_does_not_have() {
        for version in DRAFTS {
            // Arrange
            let peers =
                origin_with_track("origin-handle-unknown-subscribe", RETENTION, version).await;
            let mut subscriber = peers.server.subscriber();

            // Act
            let (subscription, ()) = within(async {
                tokio::join!(
                    subscriber.subscribe(
                        NAMESPACE.to_string(),
                        "other".to_string(),
                        subscribe_option(FilterType::NextGroupStart)
                    ),
                    async {
                        let event = peers.client.receive_event().await.unwrap();
                        let Some(SessionEvent::Subscribe(handler)) =
                            peers.origin.handle(event).await
                        else {
                            panic!("expected the SUBSCRIBE back");
                        };
                        handler
                            .error(RequestErrorCode::TrackDoesNotExist, "unknown".to_string())
                            .await
                            .unwrap();
                    }
                )
            })
            .await;

            // Assert: the caller's answer reaches the subscriber
            assert_eq!(
                rejection(subscription).code,
                RequestErrorCode::TrackDoesNotExist
            );
        }
    }

    #[tokio::test]
    async fn handle_returns_events_that_are_not_about_its_tracks() {
        for version in DRAFTS {
            // Arrange
            let peers = origin_with_track("origin-handle-other", RETENTION, version).await;
            let publisher = peers.server.publisher();

            // Act
            let (published, unhandled) = within(async {
                tokio::join!(publisher.publish_namespace("other".to_string()), async {
                    let event = peers.client.receive_event().await.unwrap();
                    let unhandled = peers.origin.handle(event).await;
                    if let Some(SessionEvent::PublishNamespace(handler)) = &unhandled {
                        handler.ok().await.unwrap();
                    }
                    unhandled
                })
            })
            .await;

            // Assert
            assert!(matches!(unhandled, Some(SessionEvent::PublishNamespace(_))));
            published.unwrap();
        }
    }

    /// A draft-18 track with dynamic groups and what its requests notify.
    async fn origin_with_dynamic_groups(name: &str) -> (Peers, Arc<Notify>) {
        let peers = origin_with_track(name, RETENTION, Version::Draft18).await;
        let requested = Arc::new(Notify::new());
        peers
            .origin
            .accept_new_group_requests(NAMESPACE, TRACK, requested.clone())
            .await
            .unwrap();
        (peers, requested)
    }

    /// Has the relay ask for a new group knowing `largest_group` and the
    /// origin handle the REQUEST_UPDATE.
    async fn request_new_group(
        peers: &Peers,
        subscription: &Subscription,
        largest_group: Option<u64>,
    ) {
        let sent = peers
            .server
            .subscriber()
            .request_new_group(subscription, largest_group)
            .await
            .unwrap();
        assert!(sent);
        let event = within(peers.client.receive_event()).await.unwrap();
        let returned = peers.origin.handle(event).await;
        assert!(matches!(returned, Some(SessionEvent::SubscribeUpdate(_))));
    }

    fn is_notified(requested: &Notify) -> bool {
        requested.notified().now_or_never().is_some()
    }

    #[tokio::test]
    async fn a_new_group_request_on_a_published_track_is_notified() {
        // Arrange
        let (peers, requested) = origin_with_dynamic_groups("origin-new-group-publish").await;
        let accepted = publish(&peers).await;

        // Act
        request_new_group(&peers, &accepted, None).await;

        // Assert
        assert!(accepted.track_properties().dynamic_groups());
        assert!(is_notified(&requested));
    }

    #[tokio::test]
    async fn a_new_group_request_for_a_group_that_began_is_not_notified() {
        // Arrange: the track is in group FIRST_GROUP_ID
        let (peers, requested) = origin_with_dynamic_groups("origin-new-group-stale").await;
        let accepted = publish(&peers).await;
        peers
            .origin
            .write(NAMESPACE, TRACK, plain(GroupBoundary::Next, b"a"))
            .await
            .unwrap();

        // Act: the subscriber knows only the group before it, then the group itself
        request_new_group(&peers, &accepted, Some(FIRST_GROUP_ID - 1)).await;
        let for_begun_group = is_notified(&requested);
        request_new_group(&peers, &accepted, Some(FIRST_GROUP_ID)).await;
        let for_next_group = is_notified(&requested);

        // Assert
        assert!(!for_begun_group);
        assert!(for_next_group);
    }

    #[tokio::test]
    async fn a_subscribe_with_a_new_group_request_is_notified() {
        // Arrange
        let (peers, requested) = origin_with_dynamic_groups("origin-new-group-subscribe").await;
        let option = SubscribeOption {
            new_group_request: Some(0),
            ..SubscribeOption::default()
        };
        let mut subscriber = peers.server.subscriber();

        // Act
        let (subscription, ()) = within(async {
            tokio::join!(
                subscriber.subscribe(NAMESPACE.to_string(), TRACK.to_string(), option),
                async {
                    let event = peers.client.receive_event().await.unwrap();
                    assert!(peers.origin.handle(event).await.is_none());
                }
            )
        })
        .await;

        // Assert
        assert!(subscription.unwrap().track_properties().dynamic_groups());
        assert!(is_notified(&requested));
    }

    #[tokio::test]
    async fn a_track_without_dynamic_groups_does_not_say_it_has_them() {
        // Arrange
        let peers = origin_with_track("origin-static-groups", RETENTION, Version::Draft18).await;

        // Act
        let accepted = publish(&peers).await;

        // Assert
        assert!(!accepted.track_properties().dynamic_groups());
    }
}
