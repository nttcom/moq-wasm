use std::{rc::Rc, time::Duration};

use bytes::Bytes;
use js_sys::{Function, Promise, Uint8Array};
use mediapack::loc::from_extension_headers;
use moqt::{
    AuthorizationTokenSource, BROWSER, ClientConfig, Endpoint, RequestFailure,
    Session as MoqtSession, Subscription as MoqtSubscription, TerminationErrorCode, TrackObject,
    TrackReader, executor,
    fetch::{Fetch, FetchDataReceiver, FetchOption},
    namespace::NamespaceEvent,
    object::{
        ContentExists, DataReceiver, ExtensionHeaders, ForwardingPreference, Location,
        MalformedTrackError, Object, StreamDataReceiver, StreamDataReceiverFactory, Subgroup,
    },
    publish::{GroupBoundary, OutgoingObject, Retention, WriteOutcome},
    subscribe::{FilterType, SubscribeOption},
    wire::{RequestKind, request_error_code_to_wire},
};
use tokio::sync::{Mutex, mpsc};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::{JsFuture, future_to_promise, spawn_local};

use crate::{
    js_error,
    loc::{encode_loc_header, parse_loc_header},
    request_rejection::request_failure_code,
    session_routes::SessionRoutes,
};

/// Matches the relay's default RELAY_CACHE_TTL_SECS, so a FETCH the relay
/// forwards for a range it never cached can still be answered here.
const DEFAULT_RETENTION: Duration = Duration::from_secs(60);

/// Requests reject with an `Error` whose `code` is the draft-14 request error
/// code.
#[wasm_bindgen]
pub struct Session {
    session: Rc<MoqtSession>,
    routes: Rc<SessionRoutes>,
    closed: Promise,
    _endpoint: Endpoint<BROWSER>,
}

#[wasm_bindgen]
impl Session {
    /// `token` is a string, a function returning a promise of one, which is
    /// called before each CLIENT_SETUP, or `undefined`. The session follows
    /// GOAWAY and reconnects by itself.
    pub async fn connect(url: String, token: JsValue) -> Result<Session, JsValue> {
        let endpoint = Endpoint::<BROWSER>::create_client(&ClientConfig {
            authorization_token: token_source(token)?,
            ..ClientConfig::default()
        })
        .map_err(anyhow_error)?;
        let session = endpoint
            .connect(&url)
            .await
            .map_err(anyhow_error)?
            .await
            .map_err(anyhow_error)?;
        let session = Rc::new(session);
        let routes = Rc::new(SessionRoutes::default());
        spawn_local({
            let session = session.clone();
            let routes = routes.clone();
            async move { routes.route(&session).await }
        });
        let closed = future_to_promise({
            let session = session.clone();
            async move {
                let close = session.closed().await;
                Ok(CloseInfo {
                    code: close.code,
                    reason: close.reason,
                }
                .into())
            }
        });
        Ok(Self {
            session,
            routes,
            closed,
            _endpoint: endpoint,
        })
    }

    /// Resolves to a `CloseInfo` once the session has ended.
    #[wasm_bindgen(getter)]
    pub fn closed(&self) -> Promise {
        self.closed.clone()
    }

    /// `filter` is `"largestObject"` (the default) or `"nextGroupStart"`.
    pub async fn subscribe(
        &self,
        namespace: Vec<String>,
        track: String,
        filter: Option<String>,
        forward: Option<bool>,
    ) -> Result<Subscription, JsValue> {
        let option = SubscribeOption {
            filter_type: filter_type(filter.as_deref())?,
            forward: forward.unwrap_or(true),
            ..SubscribeOption::default()
        };
        let mut subscriber = self.session.subscriber();
        let subscription = subscriber
            .subscribe(namespace.join("/"), track, option)
            .await
            .map_err(|error| request_error(RequestKind::Subscribe, &error))?;
        Ok(Subscription::new(self, subscription, Reading::Unstarted))
    }

    /// Subscribes and reads the group the largest object is in from its first
    /// object, through a joining FETCH (draft-14 §9.16.2).
    #[wasm_bindgen(js_name = subscribeCurrentGroup)]
    pub async fn subscribe_current_group(
        &self,
        namespace: Vec<String>,
        track: String,
    ) -> Result<Subscription, JsValue> {
        let (subscription, reader) = self
            .session
            .subscriber()
            .subscribe_current_group(namespace.join("/"), track)
            .await
            .map_err(|error| request_error(RequestKind::Subscribe, &error))?;
        Ok(Subscription::new(
            self,
            subscription,
            Reading::Objects(reader),
        ))
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
        let mut subscriber = self.session.subscriber();
        let handle = subscriber
            .fetch(
                namespace.join("/"),
                track,
                Location {
                    group_id: start_group,
                    object_id: start_object,
                },
                Location {
                    group_id: end_group,
                    object_id: end_object,
                },
                FetchOption::default(),
            )
            .await
            .map_err(|error| request_error(RequestKind::Fetch, &error))?;
        let receiver = subscriber
            .accept_fetch_receiver(&handle)
            .await
            .map_err(anyhow_error)?;
        Ok(FetchStream {
            receiver: Rc::new(Mutex::new(receiver)),
        })
    }

    /// TRACK_STATUS (draft-14 §9.20). Resolves to the track's largest
    /// location, or `undefined` while it has none.
    #[wasm_bindgen(js_name = trackStatus)]
    pub async fn track_status(
        &self,
        namespace: Vec<String>,
        track: String,
    ) -> Result<Option<MoqLocation>, JsValue> {
        let status = self
            .session
            .subscriber()
            .track_status(namespace.join("/"), track, Vec::new())
            .await
            .map_err(|error| request_error(RequestKind::TrackStatus, &error))?;
        Ok(largest(status.content_exists))
    }

    #[wasm_bindgen(js_name = watchNamespaces)]
    pub async fn watch_namespaces(&self, prefix: Vec<String>) -> Result<NamespaceWatch, JsValue> {
        let events = self.routes.watch(prefix.clone());
        self.session
            .subscriber()
            .subscribe_namespace(prefix.join("/"))
            .await
            .map_err(|error| request_error(RequestKind::SubscribeNamespace, &error))?;
        Ok(NamespaceWatch {
            session: self.session.clone(),
            prefix,
            events: Rc::new(Mutex::new(events)),
        })
    }

    /// PUBLISH_NAMESPACE (draft-14 §9.23).
    pub async fn announce(&self, namespace: Vec<String>) -> Result<Announcement, JsValue> {
        let namespace = namespace.join("/");
        self.session
            .publisher()
            .publish_namespace(namespace.clone())
            .await
            .map_err(|error| request_error(RequestKind::PublishNamespace, &error))?;
        Ok(Announcement {
            session: self.session.clone(),
            namespace,
        })
    }

    /// Registers a track to publish. The cache keeps objects for `retention_ms`
    /// (60 s by default), or only the newest one when it is 0. With `publish`,
    /// the track is also sent with PUBLISH (draft-14 §9.13); announce the
    /// namespace first.
    #[wasm_bindgen(js_name = publishTrack)]
    pub async fn publish_track(
        &self,
        namespace: Vec<String>,
        track: String,
        retention_ms: Option<f64>,
        publish: Option<bool>,
    ) -> Result<TrackWriter, JsValue> {
        let namespace = namespace.join("/");
        let retention = match retention_ms {
            None => Retention::For(DEFAULT_RETENTION),
            Some(0.0) => Retention::NewestOnly,
            Some(ms) => Retention::For(Duration::from_secs_f64(ms / 1000.0)),
        };
        let origin = &self.routes.origin;
        origin
            .ensure_track(
                &namespace,
                &track,
                executor::system_time().as_micros() as u64,
                retention,
            )
            .await;
        if publish.unwrap_or(false) {
            origin
                .publish(&self.session, &namespace, &track)
                .await
                .map_err(anyhow_error)?;
        }
        Ok(TrackWriter {
            routes: self.routes.clone(),
            namespace,
            track,
        })
    }

    pub fn close(&self) {
        self.session
            .close_with_error(TerminationErrorCode::NoError, "closed by the application");
    }
}

#[wasm_bindgen]
pub struct Announcement {
    session: Rc<MoqtSession>,
    namespace: String,
}

#[wasm_bindgen]
impl Announcement {
    /// PUBLISH_NAMESPACE_DONE (draft-14 §9.26).
    pub async fn close(&self) -> Result<(), JsValue> {
        self.session
            .publisher()
            .publish_namespace_done(self.namespace.clone())
            .await
            .map_err(anyhow_error)
    }
}

#[wasm_bindgen]
pub struct TrackWriter {
    routes: Rc<SessionRoutes>,
    namespace: String,
    track: String,
}

#[wasm_bindgen]
impl TrackWriter {
    /// `loc_header` is the object's LOC properties (draft-ietf-moq-loc-04).
    /// Resolves to whether a subscriber took the object; it is cached either
    /// way. An object within a group that was never started is dropped.
    pub async fn write(
        &self,
        payload: Vec<u8>,
        new_group: bool,
        loc_header: JsValue,
    ) -> Result<bool, JsValue> {
        let group = if new_group {
            GroupBoundary::Next
        } else {
            GroupBoundary::Within
        };
        let object = OutgoingObject {
            group,
            extension_headers: parse_loc_header(loc_header)?,
            payload: Bytes::from(payload),
        };
        let outcome = self
            .routes
            .origin
            .write(&self.namespace, &self.track, object)
            .await
            .map_err(anyhow_error)?;
        Ok(matches!(outcome, WriteOutcome::Sent(_)))
    }

    /// Sends `payload` as a group of its own, the layout of an MSF catalog
    /// (draft-ietf-moq-msf-01 §5).
    #[wasm_bindgen(js_name = writeGroup)]
    pub async fn write_group(&self, payload: Vec<u8>) -> Result<(), JsValue> {
        self.routes
            .origin
            .write_group(&self.namespace, &self.track, Bytes::from(payload))
            .await
            .map(|_| ())
            .map_err(anyhow_error)
    }
}

/// One SUBSCRIBE (draft-14 §9.7).
#[wasm_bindgen]
pub struct Subscription {
    session: Rc<MoqtSession>,
    routes: Rc<SessionRoutes>,
    subscription: MoqtSubscription,
    reading: Rc<Mutex<Reading>>,
    done: Promise,
}

enum Reading {
    Unstarted,
    Objects(TrackReader),
    Groups(StreamDataReceiverFactory),
}

impl Subscription {
    fn new(session: &Session, subscription: MoqtSubscription, reading: Reading) -> Self {
        let done = session.routes.publish_done(subscription.request_id());
        Self {
            session: session.session.clone(),
            routes: session.routes.clone(),
            subscription,
            reading: Rc::new(Mutex::new(reading)),
            done: future_to_promise(async move {
                Ok(match done.await {
                    Ok(done) => PublishDoneInfo {
                        code: done.code,
                        reason: done.reason,
                    }
                    .into(),
                    Err(_) => JsValue::UNDEFINED,
                })
            }),
        }
    }
}

#[wasm_bindgen]
impl Subscription {
    /// The largest location SUBSCRIBE_OK reported.
    #[wasm_bindgen(getter)]
    pub fn largest(&self) -> Option<MoqLocation> {
        largest(self.subscription.content_exists())
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
        let reading = self.reading.clone();
        let mut reading = reading.lock().await;
        if let Reading::Unstarted = *reading {
            *reading = Reading::Objects(self.session.subscriber().track_reader(&self.subscription));
        }
        let Reading::Objects(reader) = &mut *reading else {
            return Err(js_error("the subscription is read by group"));
        };
        let object = reader.next_object().await.map_err(anyhow_error)?;
        Ok(object.map(MoqObject::from))
    }

    /// The next subgroup stream; `undefined` once the track has no more
    /// streams. Rejects for a track delivered in datagrams, and once `next` read
    /// the subscription.
    #[wasm_bindgen(js_name = nextGroup)]
    pub async fn next_group(&self) -> Result<Option<Group>, JsValue> {
        let reading = self.reading.clone();
        let mut reading = reading.lock().await;
        if let Reading::Unstarted = *reading {
            let mut subscriber = self.session.subscriber();
            match subscriber.accept_data_receiver(&self.subscription).await {
                Ok(DataReceiver::Stream(factory)) => *reading = Reading::Groups(factory),
                Ok(DataReceiver::Datagram(_)) => {
                    return Err(anyhow_error(MalformedTrackError.into()));
                }
                Err(_) => return Ok(None),
            }
        }
        let Reading::Groups(factory) = &mut *reading else {
            return Err(js_error("the subscription is read by object"));
        };
        let mut receiver = match factory.next().await {
            Ok(receiver) => receiver,
            Err(error) if error.is::<MalformedTrackError>() => return Err(anyhow_error(error)),
            Err(_) => return Ok(None),
        };
        match receiver
            .receive()
            .await
            .map_err(|error| js_error(error.to_string()))?
        {
            Some(Subgroup::Header(start)) => Ok(Some(Group {
                group_id: start.group_id,
                subgroup_id: start.subgroup_id,
                receiver: Rc::new(Mutex::new(receiver)),
            })),
            Some(Subgroup::Object(_)) | None => {
                Err(js_error("a subgroup stream did not start with its header"))
            }
        }
    }

    /// SUBSCRIBE_UPDATE with the Forward flag (draft-14 §9.10).
    #[wasm_bindgen(js_name = setForward)]
    pub async fn set_forward(&self, forward: bool) -> Result<(), JsValue> {
        self.session
            .subscriber()
            .update_forward(&self.subscription, forward)
            .await
            .map_err(anyhow_error)
    }

    /// SUBSCRIBE_UPDATE (draft-14 §9.10).
    #[wasm_bindgen(js_name = endBeforeGroup)]
    pub async fn end_before_group(&self, group_id: u64, forward: bool) -> Result<(), JsValue> {
        self.session
            .subscriber()
            .end_before_group(&self.subscription, group_id, forward)
            .await
            .map_err(anyhow_error)
    }

    pub async fn close(&self) -> Result<(), JsValue> {
        self.routes.forget(self.subscription.request_id());
        self.session
            .subscriber()
            .unsubscribe(&self.subscription)
            .await
            .map_err(anyhow_error)
    }
}

#[wasm_bindgen]
pub struct FetchStream {
    receiver: Rc<Mutex<FetchDataReceiver>>,
}

#[wasm_bindgen]
impl FetchStream {
    /// The next object with a payload; `undefined` once the response ends.
    /// Rejects when the publisher resets the stream.
    pub async fn next(&self) -> Result<Option<MoqObject>, JsValue> {
        let receiver = self.receiver.clone();
        let mut receiver = receiver.lock().await;
        loop {
            match receiver
                .receive()
                .await
                .map_err(|error| js_error(error.to_string()))?
            {
                Fetch::Object(object) => {
                    if let Some(object) = TrackObject::from_object(object) {
                        return Ok(Some(object.into()));
                    }
                }
                Fetch::End => return Ok(None),
            }
        }
    }
}

/// One subgroup stream of a subscription (draft-14 §10.4).
#[wasm_bindgen]
pub struct Group {
    group_id: u64,
    subgroup_id: u64,
    receiver: Rc<Mutex<StreamDataReceiver>>,
}

#[wasm_bindgen]
impl Group {
    #[wasm_bindgen(getter, js_name = groupId)]
    pub fn group_id(&self) -> u64 {
        self.group_id
    }

    #[wasm_bindgen(getter, js_name = subgroupId)]
    pub fn subgroup_id(&self) -> u64 {
        self.subgroup_id
    }

    /// The next object, a status object such as End of Group included;
    /// `undefined` once the publisher finished the stream. Rejects when the
    /// stream was reset.
    pub async fn next(&self) -> Result<Option<MoqObject>, JsValue> {
        let receiver = self.receiver.clone();
        let mut receiver = receiver.lock().await;
        loop {
            match receiver
                .receive()
                .await
                .map_err(|error| js_error(error.to_string()))?
            {
                Some(Subgroup::Object(object)) => {
                    return Ok(Some(MoqObject::from_subgroup(object)));
                }
                Some(Subgroup::Header(_)) => continue,
                None => return Ok(None),
            }
        }
    }
}

/// The namespaces published under one SUBSCRIBE_NAMESPACE prefix.
#[wasm_bindgen]
pub struct NamespaceWatch {
    session: Rc<MoqtSession>,
    prefix: Vec<String>,
    events: Rc<Mutex<mpsc::UnboundedReceiver<NamespaceEvent>>>,
}

#[wasm_bindgen]
impl NamespaceWatch {
    /// The next namespace published or withdrawn; `undefined` once the
    /// session ends.
    pub async fn next(&self) -> Option<NamespaceChange> {
        let events = self.events.clone();
        let event = events.lock().await.recv().await?;
        Some(NamespaceChange {
            namespace: event.namespace,
            active: event.active,
        })
    }

    pub async fn close(&self) -> Result<(), JsValue> {
        self.events.lock().await.close();
        self.session
            .subscriber()
            .unsubscribe_namespace(self.prefix.join("/"))
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
pub struct MoqObject {
    group_id: u64,
    subgroup_id: u64,
    object_id: u64,
    status: Option<u8>,
    payload: Bytes,
    extension_headers: ExtensionHeaders,
}

#[wasm_bindgen]
impl MoqObject {
    #[wasm_bindgen(getter, js_name = groupId)]
    pub fn group_id(&self) -> u64 {
        self.group_id
    }

    #[wasm_bindgen(getter, js_name = subgroupId)]
    pub fn subgroup_id(&self) -> u64 {
        self.subgroup_id
    }

    #[wasm_bindgen(getter, js_name = objectId)]
    pub fn object_id(&self) -> u64 {
        self.object_id
    }

    /// The draft-14 Object Status (§10.2.1.1); `undefined` for an object with a
    /// payload.
    #[wasm_bindgen(getter)]
    pub fn status(&self) -> Option<u8> {
        self.status
    }

    #[wasm_bindgen(getter)]
    pub fn payload(&self) -> Uint8Array {
        Uint8Array::from(&self.payload[..])
    }

    /// The LOC properties of the object's extension headers
    /// (draft-ietf-moq-loc-04).
    #[wasm_bindgen(getter, js_name = locHeader)]
    pub fn loc_header(&self) -> Result<JsValue, JsValue> {
        encode_loc_header(&from_extension_headers(&self.extension_headers))
    }
}

impl From<TrackObject> for MoqObject {
    fn from(object: TrackObject) -> Self {
        Self {
            group_id: object.group_id,
            subgroup_id: object.subgroup_id,
            object_id: object.object_id,
            status: None,
            payload: object.payload,
            extension_headers: object.extension_headers,
        }
    }
}

impl MoqObject {
    fn from_subgroup(object: Object) -> Self {
        let subgroup_id = match object.forwarding {
            ForwardingPreference::Subgroup { subgroup_id } => subgroup_id,
            ForwardingPreference::Datagram => 0,
        };
        Self {
            group_id: object.location.group_id,
            subgroup_id,
            object_id: object.location.object_id,
            status: object.payload.is_empty().then_some(object.status as u8),
            payload: object.payload,
            extension_headers: object.extension_headers,
        }
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

fn token_source(token: JsValue) -> Result<Option<AuthorizationTokenSource>, JsValue> {
    if token.is_undefined() || token.is_null() {
        return Ok(None);
    }
    if let Some(token) = token.as_string() {
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
            token
                .as_string()
                .ok_or_else(|| anyhow::anyhow!("the token provider resolved to a non-string"))
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

fn largest(content_exists: ContentExists) -> Option<MoqLocation> {
    match content_exists {
        ContentExists::True { location } => Some(MoqLocation {
            group_id: location.group_id,
            object_id: location.object_id,
        }),
        ContentExists::False => None,
    }
}

fn request_error(kind: RequestKind, error: &RequestFailure) -> JsValue {
    let (code, reason) = request_failure_code(error);
    let js_error = js_sys::Error::new(&reason);
    let _ = js_sys::Reflect::set(
        &js_error,
        &JsValue::from_str("code"),
        &JsValue::from(request_error_code_to_wire(code, kind)),
    );
    js_error.into()
}

fn anyhow_error(error: anyhow::Error) -> JsValue {
    js_error(format!("{error:#}"))
}
