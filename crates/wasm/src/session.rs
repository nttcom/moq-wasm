use std::{sync::Arc, time::Duration};

use js_sys::{Function, Promise, Uint8Array};
use mediapack::loc::from_extension_headers;
use moqt::{
    AuthorizationTokenSource, BROWSER, ClientConfig, Endpoint, TerminationErrorCode, Version,
    client::{
        self, FetchError, GroupStartOptions, PublishTrackOptions, ReceivedObject, RequestError,
        SubscribeOptions,
    },
    object::{GroupOrder, Location},
    publish::Retention,
    subscribe::FilterType,
    wire::{publish_done_code_to_wire, request_error_code_to_wire},
};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::{JsFuture, future_to_promise};

use broadcast::RelayConnection;

use crate::{
    anyhow_error, js_error,
    loc::{encode_loc_header, parse_loc_header},
    log_sink::SessionScope,
    media_broadcast::MediaBroadcast,
    mp4::Mp4Index,
    mp4_broadcast::Mp4Broadcast,
};

/// Requests reject with an `Error` whose `code` is the draft-14 request error
/// code.
#[wasm_bindgen]
pub struct Session {
    session: client::Session,
    scope: SessionScope,
    closed: Promise,
    _endpoint: Endpoint<BROWSER>,
}

#[wasm_bindgen]
impl Session {
    /// `token` is a string, a function returning a promise of one, which is
    /// called before each CLIENT_SETUP, or `undefined`. The session follows
    /// GOAWAY and reconnects by itself. `wasm_load_wait_ms` is how long the
    /// caller waited for this module to load. `versions` names the drafts to
    /// offer, most preferred first, each `draft-14` or `draft-18`;
    /// `undefined` offers draft-18, then draft-14, and an unknown name rejects.
    pub async fn connect(
        url: String,
        token: JsValue,
        wasm_load_wait_ms: f64,
        versions: Option<Vec<String>>,
    ) -> Result<Session, JsValue> {
        let versions = Version::offered(versions.as_deref()).map_err(anyhow_error)?;
        let scope = SessionScope::new();
        scope
            .scoped(connect_session(
                url,
                token,
                wasm_load_wait_ms,
                versions,
                scope,
            ))
            .await
    }

    /// `"webtransport"`, or `"websocket"` once the session fell back to QMux
    /// over WebSocket.
    #[wasm_bindgen(getter)]
    pub fn transport(&self) -> String {
        match self.session.moqt_session().url_scheme() {
            "wss" => "websocket",
            _ => "webtransport",
        }
        .to_string()
    }

    /// Resolves to a `CloseInfo` once the session has ended.
    #[wasm_bindgen(getter)]
    pub fn closed(&self) -> Promise {
        self.closed.clone()
    }

    /// The newest token the session connected with, which its TypeScript logs
    /// are handed to `setLogSink`'s callbacks with; `undefined` without one.
    #[wasm_bindgen(getter)]
    pub fn token(&self) -> Option<String> {
        self.scope.token()
    }

    #[wasm_bindgen(js_name = transportStats)]
    pub fn transport_stats(&self) -> TransportStats {
        let bytes = self.session.transferred_bytes();
        TransportStats {
            sent_bytes: bytes.sent as f64,
            received_bytes: bytes.received as f64,
        }
    }

    /// `filter` is `"largestObject"` (the default) or `"nextGroupStart"`.
    /// `priority` is the Subscriber Priority (draft-14 §7.2), 128 by default:
    /// the relay sends a lower value first. `delivery_timeout_ms` is the
    /// DELIVERY TIMEOUT (draft-14 §9.2.1.2): the relay stops sending an object that
    /// many milliseconds after receiving it; without it every object is sent.
    /// `group_order` is `"ascending"` (the default) or `"descending"` (§7).
    #[allow(clippy::too_many_arguments)]
    pub async fn subscribe(
        &self,
        namespace: Vec<String>,
        track: String,
        filter: Option<String>,
        forward: Option<bool>,
        priority: Option<u8>,
        delivery_timeout_ms: Option<u32>,
        group_order: Option<String>,
    ) -> Result<Subscription, JsValue> {
        self.scope
            .scoped(async {
                let delivery =
                    group_start_options(priority, delivery_timeout_ms, group_order.as_deref())?;
                let options = SubscribeOptions {
                    filter: filter_type(filter.as_deref())?,
                    forward: forward.unwrap_or(true),
                    priority: delivery.priority,
                    delivery_timeout: delivery.delivery_timeout,
                    group_order: delivery.group_order,
                };
                let subscription = self
                    .session
                    .subscribe(&namespace, &track, options)
                    .await
                    .map_err(request_error)?;
                Ok(Subscription::new(subscription, self.scope))
            })
            .await
    }

    /// Subscribes and reads the group the largest object is in from its first
    /// object, through a joining FETCH (draft-14 §9.16.2). `priority`,
    /// `delivery_timeout_ms` and `group_order` are as in `subscribe`.
    #[wasm_bindgen(js_name = subscribeCurrentGroup)]
    pub async fn subscribe_current_group(
        &self,
        namespace: Vec<String>,
        track: String,
        priority: Option<u8>,
        delivery_timeout_ms: Option<u32>,
        group_order: Option<String>,
    ) -> Result<Subscription, JsValue> {
        self.scope
            .scoped(async {
                let options =
                    group_start_options(priority, delivery_timeout_ms, group_order.as_deref())?;
                let subscription = self
                    .session
                    .subscribe_current_group(&namespace, &track, options)
                    .await
                    .map_err(request_error)?;
                Ok(Subscription::new(subscription, self.scope))
            })
            .await
    }

    /// Subscribes and reads the track from the first object of a group
    /// (draft-18 §5.1.3.1), chosen once the SUBSCRIBE_OK arrives: the
    /// current group, through a joining FETCH, or, when the track has
    /// dynamic groups and its largest object ID is `new_group_after` or
    /// more, a new group it asks the publisher to start (NEW_GROUP_REQUEST,
    /// §10.2.13). `priority`, `delivery_timeout_ms` and `group_order` are as in
    /// `subscribe`.
    #[wasm_bindgen(js_name = subscribeGroupStart)]
    pub async fn subscribe_group_start(
        &self,
        namespace: Vec<String>,
        track: String,
        new_group_after: u32,
        priority: Option<u8>,
        delivery_timeout_ms: Option<u32>,
        group_order: Option<String>,
    ) -> Result<Subscription, JsValue> {
        self.scope
            .scoped(async {
                let options =
                    group_start_options(priority, delivery_timeout_ms, group_order.as_deref())?;
                let subscription = self
                    .session
                    .subscribe_group_start(&namespace, &track, new_group_after.into(), options)
                    .await
                    .map_err(request_error)?;
                Ok(Subscription::new(subscription, self.scope))
            })
            .await
    }

    /// A Standalone FETCH (draft-14 §9.16.1). `end_object` is the End
    /// Location's Object: the last object plus 1, and 0 means the whole group.
    #[allow(clippy::too_many_arguments)]
    pub async fn fetch(
        &self,
        namespace: Vec<String>,
        track: String,
        start_group: u64,
        start_object: u64,
        end_group: u64,
        end_object: u64,
    ) -> Result<FetchStream, JsValue> {
        self.scope
            .scoped(async {
                let fetch = self
                    .session
                    .fetch(
                        &namespace,
                        &track,
                        Location {
                            group_id: start_group,
                            object_id: start_object,
                        },
                        Location {
                            group_id: end_group,
                            object_id: end_object,
                        },
                    )
                    .await
                    .map_err(|error| match error {
                        FetchError::Request(error) => request_error(error),
                        error => anyhow_error(error.into()),
                    })?;
                Ok(FetchStream {
                    fetch,
                    scope: self.scope,
                })
            })
            .await
    }

    /// TRACK_STATUS (draft-14 §9.20). Resolves to the track's largest
    /// location, or `undefined` while it has none.
    #[wasm_bindgen(js_name = trackStatus)]
    pub async fn track_status(
        &self,
        namespace: Vec<String>,
        track: String,
    ) -> Result<Option<MoqLocation>, JsValue> {
        self.scope
            .scoped(async {
                let largest = self
                    .session
                    .track_status(&namespace, &track)
                    .await
                    .map_err(request_error)?;
                Ok(largest.map(MoqLocation::from))
            })
            .await
    }

    #[wasm_bindgen(js_name = watchNamespaces)]
    pub async fn watch_namespaces(&self, prefix: Vec<String>) -> Result<NamespaceWatch, JsValue> {
        self.scope
            .scoped(async {
                let watch = self
                    .session
                    .watch_namespaces(&prefix)
                    .await
                    .map_err(request_error)?;
                Ok(NamespaceWatch {
                    watch,
                    scope: self.scope,
                })
            })
            .await
    }

    /// PUBLISH_NAMESPACE (draft-14 §9.23).
    pub async fn announce(&self, namespace: Vec<String>) -> Result<Announcement, JsValue> {
        self.scope
            .scoped(async {
                let announcement = self
                    .session
                    .announce(&namespace)
                    .await
                    .map_err(request_error)?;
                Ok(Announcement {
                    announcement,
                    scope: self.scope,
                })
            })
            .await
    }

    /// Registers a track to publish. The cache keeps objects for `retention_ms`
    /// (60 s by default), or only the newest one when it is 0. With `publish`,
    /// the track is also sent with PUBLISH (draft-14 §9.13); announce the
    /// namespace first. With `datagram`, each object is sent in its own
    /// datagram (draft-14 §10.3) instead of on subgroup streams; rejects for a
    /// track already written to on streams. `priority` is the Publisher
    /// Priority (draft-14 §7.1), 128 by default.
    #[wasm_bindgen(js_name = publishTrack)]
    pub async fn publish_track(
        &self,
        namespace: Vec<String>,
        track: String,
        retention_ms: Option<f64>,
        publish: Option<bool>,
        datagram: Option<bool>,
        priority: Option<u8>,
    ) -> Result<TrackWriter, JsValue> {
        self.scope
            .scoped(async {
                let defaults = PublishTrackOptions::default();
                let options = PublishTrackOptions {
                    retention: match retention_ms {
                        None => defaults.retention,
                        Some(0.0) => Retention::NewestOnly,
                        Some(ms) => Retention::For(Duration::from_secs_f64(ms / 1000.0)),
                    },
                    publish: publish.unwrap_or(false),
                    datagram: datagram.unwrap_or(false),
                    priority: priority.unwrap_or(defaults.priority),
                };
                let writer = self
                    .session
                    .publish_track(&namespace, &track, options)
                    .await
                    .map_err(anyhow_error)?;
                Ok(TrackWriter {
                    writer,
                    scope: self.scope,
                })
            })
            .await
    }

    /// The broadcast lists LOC tracks only.
    pub fn broadcast(&self, namespace: Vec<String>) -> MediaBroadcast {
        self.scope
            .in_scope(|| MediaBroadcast::new(self.relay_connection(), namespace, self.scope))
    }

    /// Fails when `index` has no H.264 video.
    #[wasm_bindgen(js_name = mp4Broadcast)]
    pub fn mp4_broadcast(
        &self,
        namespace: Vec<String>,
        index: &Mp4Index,
    ) -> Result<Mp4Broadcast, JsValue> {
        self.scope
            .in_scope(|| Mp4Broadcast::new(self.relay_connection(), namespace, index, self.scope))
    }

    pub fn close(&self) {
        self.scope.in_scope(|| {
            self.session
                .moqt_session()
                .close_with_error(TerminationErrorCode::NoError, "closed by the application")
        });
    }
}

impl Session {
    /// Answers the session's events before its `Origin` does; the handler is
    /// never removed, so it lives as long as the session.
    // The browser's RelayConnection is !Send, but wasm32 runs on one thread.
    #[allow(clippy::arc_with_non_send_sync)]
    fn relay_connection(&self) -> RelayConnection {
        let connection = RelayConnection::with_client_session(&self.session);
        self.session.add_handler(Arc::new(connection.clone()));
        connection
    }
}

#[wasm_bindgen]
pub struct Announcement {
    announcement: client::Announcement,
    scope: SessionScope,
}

#[wasm_bindgen]
impl Announcement {
    /// PUBLISH_NAMESPACE_DONE (draft-14 §9.26).
    pub async fn close(&self) -> Result<(), JsValue> {
        self.scope
            .scoped(self.announcement.close())
            .await
            .map_err(anyhow_error)
    }
}

#[wasm_bindgen]
pub struct TrackWriter {
    writer: client::TrackWriter,
    scope: SessionScope,
}

#[wasm_bindgen]
impl TrackWriter {
    /// `loc_header` is the object's LOC properties (draft-ietf-moq-loc-04).
    /// Resolves to the object's group id when a subscriber took it, and to
    /// `undefined` otherwise; it is cached either way. An object within a
    /// group that was never started is dropped.
    pub async fn write(
        &self,
        payload: Vec<u8>,
        new_group: bool,
        loc_header: JsValue,
    ) -> Result<Option<u64>, JsValue> {
        self.scope
            .scoped(async {
                let extension_headers = parse_loc_header(loc_header)?;
                self.writer
                    .write(payload.into(), new_group, extension_headers)
                    .await
                    .map_err(anyhow_error)
            })
            .await
    }

    /// Sends `payload` as a group of its own, the layout of an MSF catalog
    /// (draft-ietf-moq-msf-02 §5). Resolves to the group's id when a
    /// subscriber took it, and to `undefined` otherwise.
    #[wasm_bindgen(js_name = writeGroup)]
    pub async fn write_group(&self, payload: Vec<u8>) -> Result<Option<u64>, JsValue> {
        self.scope
            .scoped(self.writer.write_group(payload.into()))
            .await
            .map_err(anyhow_error)
    }

    /// Resolves to `true` once a SUBSCRIBE for the track is answered, and to
    /// `false` once the session has ended: the moment to send what a new
    /// subscriber needs, since its SUBSCRIBE starts after the largest object.
    #[wasm_bindgen(js_name = nextSubscriber)]
    pub async fn next_subscriber(&self) -> bool {
        self.writer.next_subscriber().await
    }
}

/// One SUBSCRIBE (draft-14 §9.7).
#[wasm_bindgen]
pub struct Subscription {
    subscription: client::Subscription,
    scope: SessionScope,
    done: Promise,
}

impl Subscription {
    fn new(subscription: client::Subscription, scope: SessionScope) -> Self {
        let done = subscription.done();
        Self {
            subscription,
            scope,
            done: future_to_promise(scope.scoped(async move {
                Ok(match done.await {
                    Some(done) => PublishDoneInfo {
                        code: publish_done_code_to_wire(done.code),
                        reason: done.reason,
                    }
                    .into(),
                    None => JsValue::UNDEFINED,
                })
            })),
        }
    }
}

#[wasm_bindgen]
impl Subscription {
    /// The largest location SUBSCRIBE_OK reported.
    #[wasm_bindgen(getter)]
    pub fn largest(&self) -> Option<MoqLocation> {
        self.subscription.largest().map(MoqLocation::from)
    }

    /// Resolves to a `PublishDoneInfo` once the publisher ends the
    /// subscription, or to `undefined` once the session ends without one.
    #[wasm_bindgen(getter)]
    pub fn done(&self) -> Promise {
        self.done.clone()
    }

    /// The next object; `undefined` once the track has no more streams.
    /// Objects of one subgroup come in order; status objects are skipped.
    /// Rejects once `nextGroup` read the subscription.
    pub async fn next(&self) -> Result<Option<MoqObject>, JsValue> {
        self.scope
            .scoped(self.subscription.next())
            .await
            .map(|object| object.map(MoqObject))
            .map_err(anyhow_error)
    }

    /// The next subgroup stream; `undefined` once the track has no more
    /// streams. Rejects for a track delivered in datagrams, and once `next` read
    /// the subscription.
    #[wasm_bindgen(js_name = nextGroup)]
    pub async fn next_group(&self) -> Result<Option<Group>, JsValue> {
        self.scope
            .scoped(self.subscription.next_group())
            .await
            .map(|group| {
                group.map(|group| Group {
                    group,
                    scope: self.scope,
                })
            })
            .map_err(anyhow_error)
    }

    /// SUBSCRIBE_UPDATE with the Forward flag (draft-14 §9.10).
    #[wasm_bindgen(js_name = setForward)]
    pub async fn set_forward(&self, forward: bool) -> Result<(), JsValue> {
        self.scope
            .scoped(self.subscription.set_forward(forward))
            .await
            .map_err(anyhow_error)
    }

    /// SUBSCRIBE_UPDATE with the Subscriber Priority (draft-14 §7.1), by
    /// which the relay orders what it has not sent yet.
    #[wasm_bindgen(js_name = setPriority)]
    pub async fn set_priority(&self, priority: u8) -> Result<(), JsValue> {
        self.scope
            .scoped(self.subscription.set_priority(priority))
            .await
            .map_err(anyhow_error)
    }

    /// Asks the publisher to start a new group (NEW_GROUP_REQUEST, draft-18
    /// §10.2.13). `largest_group` is the largest group ID of the track the
    /// caller has seen. Resolves to false, without sending, for a track
    /// without dynamic groups.
    #[wasm_bindgen(js_name = requestNewGroup)]
    pub async fn request_new_group(&self, largest_group: Option<u64>) -> Result<bool, JsValue> {
        self.scope
            .scoped(self.subscription.request_new_group(largest_group))
            .await
            .map_err(anyhow_error)
    }

    /// SUBSCRIBE_UPDATE (draft-14 §9.10).
    #[wasm_bindgen(js_name = endBeforeGroup)]
    pub async fn end_before_group(&self, group_id: u64) -> Result<(), JsValue> {
        self.scope
            .scoped(self.subscription.end_before_group(group_id))
            .await
            .map_err(anyhow_error)
    }

    pub async fn close(&self) -> Result<(), JsValue> {
        self.scope
            .scoped(self.subscription.close())
            .await
            .map_err(anyhow_error)
    }
}

#[wasm_bindgen]
pub struct FetchStream {
    fetch: client::Fetch,
    scope: SessionScope,
}

#[wasm_bindgen]
impl FetchStream {
    /// The next object with a payload; `undefined` once the response ends.
    /// Rejects when the publisher resets the stream.
    pub async fn next(&self) -> Result<Option<MoqObject>, JsValue> {
        self.scope
            .scoped(self.fetch.next())
            .await
            .map(|object| object.map(MoqObject))
            .map_err(anyhow_error)
    }
}

/// One subgroup stream of a subscription (draft-14 §10.4).
#[wasm_bindgen]
pub struct Group {
    group: client::Group,
    scope: SessionScope,
}

#[wasm_bindgen]
impl Group {
    #[wasm_bindgen(getter, js_name = groupId)]
    pub fn group_id(&self) -> u64 {
        self.group.group_id()
    }

    #[wasm_bindgen(getter, js_name = subgroupId)]
    pub fn subgroup_id(&self) -> u64 {
        self.group.subgroup_id()
    }

    /// The next object, a status object such as End of Group included;
    /// `undefined` once the publisher finished the stream. Rejects when the
    /// stream was reset.
    pub async fn next(&self) -> Result<Option<MoqObject>, JsValue> {
        self.scope
            .scoped(self.group.next())
            .await
            .map(|object| object.map(MoqObject))
            .map_err(anyhow_error)
    }
}

/// The namespaces published under one SUBSCRIBE_NAMESPACE prefix.
#[wasm_bindgen]
pub struct NamespaceWatch {
    watch: client::NamespaceWatch,
    scope: SessionScope,
}

#[wasm_bindgen]
impl NamespaceWatch {
    /// The next namespace published or withdrawn; `undefined` once the
    /// session ends or `close` is called.
    pub async fn next(&self) -> Option<NamespaceChange> {
        let event = self.watch.next().await?;
        Some(NamespaceChange {
            namespace: event.namespace,
            active: event.active,
        })
    }

    /// UNSUBSCRIBE_NAMESPACE; a `next` that waits resolves to `undefined`.
    pub async fn close(&self) -> Result<(), JsValue> {
        self.scope
            .scoped(self.watch.close())
            .await
            .map_err(anyhow_error)
    }
}

#[wasm_bindgen]
pub struct NamespaceChange {
    #[wasm_bindgen(getter_with_clone, readonly)]
    pub namespace: Vec<String>,
    /// `false` once the namespace was withdrawn (PUBLISH_NAMESPACE_DONE).
    #[wasm_bindgen(readonly)]
    pub active: bool,
}

#[wasm_bindgen]
pub struct MoqObject(ReceivedObject);

#[wasm_bindgen]
impl MoqObject {
    #[wasm_bindgen(getter, js_name = groupId)]
    pub fn group_id(&self) -> u64 {
        self.0.group_id
    }

    #[wasm_bindgen(getter, js_name = subgroupId)]
    pub fn subgroup_id(&self) -> u64 {
        self.0.subgroup_id
    }

    #[wasm_bindgen(getter, js_name = objectId)]
    pub fn object_id(&self) -> u64 {
        self.0.object_id
    }

    /// The draft-14 Object Status (§10.2.1.1); `undefined` for an object with a
    /// payload.
    #[wasm_bindgen(getter)]
    pub fn status(&self) -> Option<u8> {
        self.0.status.map(|status| status as u8)
    }

    #[wasm_bindgen(getter)]
    pub fn payload(&self) -> Uint8Array {
        Uint8Array::from(&self.0.payload[..])
    }

    /// The LOC properties of the object's extension headers
    /// (draft-ietf-moq-loc-04).
    #[wasm_bindgen(getter, js_name = locHeader)]
    pub fn loc_header(&self) -> Result<JsValue, JsValue> {
        encode_loc_header(&from_extension_headers(&self.0.extension_headers))
    }
}

#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct MoqLocation {
    #[wasm_bindgen(readonly, js_name = groupId)]
    pub group_id: u64,
    #[wasm_bindgen(readonly, js_name = objectId)]
    pub object_id: u64,
}

impl From<Location> for MoqLocation {
    fn from(location: Location) -> Self {
        Self {
            group_id: location.group_id,
            object_id: location.object_id,
        }
    }
}

#[wasm_bindgen]
pub struct PublishDoneInfo {
    /// The draft-14 PUBLISH_DONE status code (§9.12).
    #[wasm_bindgen(readonly)]
    pub code: u64,
    #[wasm_bindgen(getter_with_clone, readonly)]
    pub reason: String,
}

#[wasm_bindgen]
pub struct CloseInfo {
    /// The termination code the session closed with, `undefined` when the
    /// connection was lost without one.
    #[wasm_bindgen(readonly)]
    pub code: Option<u32>,
    #[wasm_bindgen(getter_with_clone, readonly)]
    pub reason: String,
}

/// Bytes are a `number` so a page can compute rates from them; one connection
/// carries far fewer than 2^53 bytes.
#[wasm_bindgen]
pub struct TransportStats {
    #[wasm_bindgen(readonly, js_name = sentBytes)]
    pub sent_bytes: f64,
    #[wasm_bindgen(readonly, js_name = receivedBytes)]
    pub received_bytes: f64,
}

async fn connect_session(
    url: String,
    token: JsValue,
    wasm_load_wait_ms: f64,
    versions: Vec<Version>,
    scope: SessionScope,
) -> Result<Session, JsValue> {
    let authorization_token = token_source(token, scope)?;
    tracing::info!(%url, wasm_load_wait_ms, supported_versions = ?versions, "session connect requested");
    let endpoint = Endpoint::<BROWSER>::create_client(&ClientConfig {
        authorization_token,
        versions,
        ..ClientConfig::default()
    })
    .map_err(anyhow_error)?;
    let session = endpoint
        .connect(&url)
        .await
        .map_err(anyhow_error)?
        .await
        .map_err(anyhow_error)?;
    let session = client::Session::new(session);
    let closed = future_to_promise(scope.scoped({
        let session = session.moqt_session().clone();
        async move {
            let close = session.closed().await;
            Ok(CloseInfo {
                code: close.code,
                reason: close.reason,
            }
            .into())
        }
    }));
    Ok(Session {
        session,
        scope,
        closed,
        _endpoint: endpoint,
    })
}

/// Gives `scope` the token, or each token the provider resolves to.
fn token_source(
    token: JsValue,
    scope: SessionScope,
) -> Result<Option<AuthorizationTokenSource>, JsValue> {
    if token.is_undefined() || token.is_null() {
        return Ok(None);
    }
    if let Some(token) = token.as_string() {
        scope.set_token(token.clone());
        return Ok(Some(token.into()));
    }
    let Some(provider) = token.dyn_ref::<Function>().cloned() else {
        return Err(js_error("token must be a string or a function"));
    };
    Ok(Some(AuthorizationTokenSource::provider(move || {
        let provider = provider.clone();
        async move {
            let returned = provider
                .call0(&JsValue::NULL)
                .map_err(|error| anyhow::anyhow!("the token provider threw: {error:?}"))?;
            let token = JsFuture::from(Promise::resolve(&returned))
                .await
                .map_err(|error| anyhow::anyhow!("the token provider rejected: {error:?}"))?;
            let token = token
                .as_string()
                .ok_or_else(|| anyhow::anyhow!("the token provider resolved to a non-string"))?;
            scope.set_token(token.clone());
            Ok(token)
        }
    })))
}

fn filter_type(filter: Option<&str>) -> Result<FilterType, JsValue> {
    match filter {
        None | Some("largestObject") => Ok(FilterType::LargestObject),
        Some("nextGroupStart") => Ok(FilterType::NextGroupStart),
        Some(other) => Err(js_error(format!("unknown subscribe filter: {other}"))),
    }
}

fn group_start_options(
    priority: Option<u8>,
    delivery_timeout_ms: Option<u32>,
    group_order: Option<&str>,
) -> Result<GroupStartOptions, JsValue> {
    let defaults = GroupStartOptions::default();
    Ok(GroupStartOptions {
        priority: priority.unwrap_or(defaults.priority),
        delivery_timeout: delivery_timeout_ms.map(|ms| Duration::from_millis(ms.into())),
        group_order: match group_order {
            None | Some("ascending") => GroupOrder::Ascending,
            Some("descending") => GroupOrder::Descending,
            Some(other) => return Err(js_error(format!("unknown group order: {other}"))),
        },
    })
}

fn request_error(error: RequestError) -> JsValue {
    let js_error = js_sys::Error::new(&error.reason());
    let _ = js_sys::Reflect::set(
        &js_error,
        &JsValue::from_str("code"),
        &JsValue::from(request_error_code_to_wire(error.code(), error.kind)),
    );
    js_error.into()
}
