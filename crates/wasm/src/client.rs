mod callbacks;
mod session_events;
mod track_io;

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use bytes::Bytes;
use moqt::{
    BROWSER, ClientConfig, ContentExists, Endpoint, FetchDataSender, FetchHandle, FetchHandler,
    FetchOption, FilterType, GroupOrder, Location, ObjectStatus, PublishHandler,
    PublishNamespaceHandler, PublishOption, RequestTimeoutError, Session, SubgroupId,
    SubgroupObject, SubgroupObjectSender, SubscribeHandler, SubscribeOption, SubscribeUpdateOption,
    Subscription, TerminationErrorCode, TrackStatusHandler, TransportSendError,
    wire::{
        AuthorizationToken, DatagramField, FetchObject, FetchObjectField, FetchOk, NamespaceOk,
        ObjectDatagram, ObjectDatagramPayload, PublishOk, RequestError, SubscribeOk, TrackStatusOk,
    },
};
use tokio::sync::Mutex;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use self::callbacks::{CallbackSelector, Callbacks, emit};
use crate::{
    client_state::{ClientState, TrackKey},
    js_error,
    messages::{
        FetchOkMessage, NamespaceOkMessage, PublishOkMessage, RequestErrorMessage,
        ServerSetupMessage, SubgroupState, SubscribeOkMessage,
    },
};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = log)]
    fn console_log(message: &str);
}

/// (track alias, group id, subgroup id)
type WriterKey = (u64, u64, u64);

const ERROR_INTERNAL: u64 = 0x0;
const ERROR_TIMEOUT: u64 = 0x2;

/// Request handlers the peer is waiting on until JavaScript answers them,
/// keyed by the peer's request id. A handler dropped unanswered rejects its
/// request with NOT_SUPPORTED.
#[derive(Default)]
pub(crate) struct IncomingRequests {
    pub(crate) publish_namespaces: HashMap<u64, PublishNamespaceHandler>,
    pub(crate) publishes: HashMap<u64, PublishHandler>,
    pub(crate) subscribes: HashMap<u64, SubscribeHandler>,
    pub(crate) track_statuses: HashMap<u64, TrackStatusHandler>,
    pub(crate) fetches: HashMap<u64, FetchHandler>,
}

#[derive(Default)]
pub(crate) struct ClientShared {
    in_use: Cell<bool>,
    session: RefCell<Option<Rc<Session>>>,
    pub(crate) state: RefCell<ClientState>,
    pub(crate) callbacks: RefCell<Callbacks>,
    pub(crate) incoming: RefCell<IncomingRequests>,
    /// Outgoing subscriptions keyed by the request id JavaScript chose.
    subscriptions: RefCell<HashMap<u64, Subscription>>,
    /// Request ids on the wire of every SUBSCRIBE JavaScript ever issued,
    /// keyed by the id JavaScript chose; kept after UNSUBSCRIBE so a late
    /// SUBSCRIBE_UPDATE still names a request id the peer has seen.
    subscription_wire_ids: RefCell<HashMap<u64, u64>>,
    /// Outgoing FETCH request ids on the wire keyed by the id JavaScript chose.
    pub(crate) fetch_requests: RefCell<HashMap<u64, u64>>,
    /// Subscriptions this client sends objects on, keyed by track alias.
    track_subscriptions: RefCell<HashMap<u64, Subscription>>,
    stream_senders: RefCell<HashMap<WriterKey, Rc<Mutex<SubgroupObjectSender>>>>,
    stream_object_numbers: RefCell<HashMap<WriterKey, u64>>,
    fetch_senders: RefCell<HashMap<u64, Rc<Mutex<FetchDataSender>>>>,
}

impl ClientShared {
    fn on_disconnected(&self) {
        self.clear();
        emit(&self.callbacks, |c| &c.connection_closed, &[JsValue::NULL]);
    }

    fn clear(&self) {
        self.in_use.set(false);
        self.session.borrow_mut().take();
        *self.incoming.borrow_mut() = IncomingRequests::default();
        self.subscriptions.borrow_mut().clear();
        self.subscription_wire_ids.borrow_mut().clear();
        self.fetch_requests.borrow_mut().clear();
        self.track_subscriptions.borrow_mut().clear();
        self.stream_senders.borrow_mut().clear();
        self.stream_object_numbers.borrow_mut().clear();
        self.fetch_senders.borrow_mut().clear();
    }

    fn reset_fetch_sender(&self, request_id: u64) {
        let Some(sender) = self.fetch_senders.borrow_mut().remove(&request_id) else {
            return;
        };
        spawn_local(async move {
            let _ = sender.lock().await.reset(0).await;
        });
    }
}

#[wasm_bindgen]
pub struct MOQTClient {
    url: String,
    shared: Rc<ClientShared>,
}

#[wasm_bindgen]
impl MOQTClient {
    #[wasm_bindgen(constructor)]
    pub fn new(url: String) -> Self {
        Self {
            url,
            shared: Rc::new(ClientShared::default()),
        }
    }

    pub fn url(&self) -> JsValue {
        JsValue::from_str(&self.url)
    }

    #[wasm_bindgen(js_name = onServerSetup)]
    pub fn set_server_setup_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().server_setup = Some(callback);
    }

    #[wasm_bindgen(js_name = onPublishNamespace)]
    pub fn set_publish_namespace_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().publish_namespace = Some(callback);
    }

    #[wasm_bindgen(js_name = onPublishNamespaceDone)]
    pub fn set_publish_namespace_done_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().publish_namespace_done = Some(callback);
    }

    #[wasm_bindgen(js_name = onPublishNamespaceResponse)]
    pub fn set_publish_namespace_response_callback(&mut self, callback: js_sys::Function) {
        self.shared
            .callbacks
            .borrow_mut()
            .publish_namespace_response = Some(callback);
    }

    #[wasm_bindgen(js_name = onSubscribeNamespaceResponse)]
    pub fn set_subscribe_namespace_response_callback(&mut self, callback: js_sys::Function) {
        self.shared
            .callbacks
            .borrow_mut()
            .subscribe_namespace_response = Some(callback);
    }

    #[wasm_bindgen(js_name = onPublish)]
    pub fn set_publish_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().publish = Some(callback);
    }

    #[wasm_bindgen(js_name = onPublishResponse)]
    pub fn set_publish_response_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().publish_response = Some(callback);
    }

    #[wasm_bindgen(js_name = onSubscribe)]
    pub fn set_subscribe_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().subscribe = Some(callback);
    }

    #[wasm_bindgen(js_name = onSubscribeResponse)]
    pub fn set_subscribe_response_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().subscribe_response = Some(callback);
    }

    #[wasm_bindgen(js_name = onIncomingUnsubscribe)]
    pub fn set_incoming_unsubscribe_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().incoming_unsubscribe = Some(callback);
    }

    #[wasm_bindgen(js_name = onObjectDatagram)]
    pub fn set_object_datagram_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().object_datagram = Some(callback);
    }

    #[wasm_bindgen(js_name = onObjectDatagramStatus)]
    pub fn set_object_datagram_status_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().object_datagram_status = Some(callback);
    }

    #[wasm_bindgen(js_name = onSubgroupHeader)]
    pub fn set_subgroup_header_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().subgroup_header = Some(callback);
    }

    #[wasm_bindgen(js_name = onSubgroupObject)]
    pub fn set_subgroup_object_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().subgroup_object = Some(callback);
    }

    #[wasm_bindgen(js_name = onFetch)]
    pub fn set_fetch_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().fetch = Some(callback);
    }

    #[wasm_bindgen(js_name = onFetchCancel)]
    pub fn set_fetch_cancel_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().fetch_cancel = Some(callback);
    }

    #[wasm_bindgen(js_name = onFetchResponse)]
    pub fn set_fetch_response_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().fetch_response = Some(callback);
    }

    #[wasm_bindgen(js_name = onFetchObject)]
    pub fn set_fetch_object_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().fetch_object = Some(callback);
    }

    #[wasm_bindgen(js_name = onFetchStreamEnd)]
    pub fn set_fetch_stream_end_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().fetch_stream_end = Some(callback);
    }

    #[wasm_bindgen(js_name = onTrackStatus)]
    pub fn set_track_status_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().track_status = Some(callback);
    }

    #[wasm_bindgen(js_name = onTrackStatusResponse)]
    pub fn set_track_status_response_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().track_status_response = Some(callback);
    }

    #[wasm_bindgen(js_name = onConnectionClosed)]
    pub fn set_connection_closed_callback(&mut self, callback: js_sys::Function) {
        self.shared.callbacks.borrow_mut().connection_closed = Some(callback);
    }

    #[wasm_bindgen(js_name = isConnected)]
    pub fn is_connected(&self) -> bool {
        self.shared.in_use.get()
    }

    /// The WebTransport session is opened together with the SETUP exchange in
    /// `sendClientSetup`, because the authorization token travels in
    /// CLIENT_SETUP; `start` only marks the client as in use.
    pub async fn start(&self) -> Result<(), JsValue> {
        self.shared.in_use.set(true);
        Ok(())
    }

    #[wasm_bindgen(js_name = close)]
    pub async fn close(&self) -> Result<(), JsValue> {
        // Explicit close is driven by the JS wrapper, so suppress the async
        // `onConnectionClosed` callback path to avoid re-entrant cleanup.
        self.shared.callbacks.borrow_mut().connection_closed = None;
        let session = self.shared.session.borrow_mut().take();
        if let Some(session) = session {
            session.close_with_error(TerminationErrorCode::NoError, "closed by the application");
        }
        self.shared.clear();
        Ok(())
    }

    /// `versions` and `max_request_id` are kept for the JavaScript API; the
    /// session always negotiates draft-14 with the crate's own SETUP parameters.
    #[wasm_bindgen(js_name = sendClientSetup)]
    pub async fn send_client_setup(
        &self,
        _versions: Vec<u64>,
        _max_request_id: u64,
        auth_token: Option<String>,
    ) -> Result<(), JsValue> {
        let endpoint = Endpoint::<BROWSER>::create_client(&ClientConfig {
            port: 0,
            verify_certificate: true,
            authorization_token: auth_token,
        })
        .map_err(anyhow_error)?;
        let session = endpoint
            .connect(&self.url)
            .await
            .map_err(anyhow_error)?
            .await
            .map_err(anyhow_error)?;
        let session = Rc::new(session);
        *self.shared.session.borrow_mut() = Some(session.clone());
        spawn_local(session_events::run_session_events(
            self.shared.clone(),
            session.clone(),
        ));
        if let Some(server_setup) = session.server_setup() {
            let message = ServerSetupMessage::from(server_setup);
            emit(
                &self.shared.callbacks,
                |c| &c.server_setup,
                &[message.into()],
            );
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = sendPublishNamespace)]
    pub async fn send_publish_namespace(
        &self,
        request_id: u64,
        track_namespace: Vec<String>,
        _auth_info: String,
    ) -> Result<(), JsValue> {
        if self
            .shared
            .state
            .borrow()
            .contains_published_namespace(&track_namespace)
        {
            return Ok(());
        }
        let session = self.session()?;
        self.shared
            .state
            .borrow_mut()
            .register_publish_namespace_request(request_id, track_namespace.clone());
        let result = session
            .publisher()
            .publish_namespace(track_namespace.join("/"))
            .await;
        self.shared
            .state
            .borrow_mut()
            .finish_publish_namespace_request(request_id, result.is_ok());
        self.emit_namespace_response(|c| &c.publish_namespace_response, request_id, result);
        Ok(())
    }

    #[wasm_bindgen(js_name = sendPublishNamespaceOk)]
    pub async fn send_publish_namespace_ok(&self, request_id: u64) -> Result<(), JsValue> {
        let handler = self
            .shared
            .incoming
            .borrow_mut()
            .publish_namespaces
            .remove(&request_id)
            .ok_or_else(|| js_error(format!("unknown publish namespace request: {request_id}")))?;
        handler.ok().await.map_err(send_error)
    }

    #[wasm_bindgen(js_name = sendPublishNamespaceError)]
    pub async fn send_publish_namespace_error(
        &self,
        request_id: u64,
        error_code: u64,
        reason_phrase: String,
    ) -> Result<(), JsValue> {
        let handler = self
            .shared
            .incoming
            .borrow_mut()
            .publish_namespaces
            .remove(&request_id)
            .ok_or_else(|| js_error(format!("unknown publish namespace request: {request_id}")))?;
        handler
            .error(error_code, reason_phrase)
            .await
            .map_err(send_error)
    }

    #[wasm_bindgen(js_name = sendSubscribeNamespace)]
    pub async fn send_subscribe_namespace(
        &self,
        request_id: u64,
        track_namespace_prefix: Vec<String>,
        _auth_info: String,
    ) -> Result<(), JsValue> {
        if self
            .shared
            .state
            .borrow()
            .contains_subscribed_namespace_prefix(&track_namespace_prefix)
        {
            return Ok(());
        }
        let session = self.session()?;
        self.shared
            .state
            .borrow_mut()
            .register_subscribe_namespace_request(request_id, track_namespace_prefix.clone());
        let result = session
            .subscriber()
            .subscribe_namespace(track_namespace_prefix.join("/"))
            .await;
        self.shared
            .state
            .borrow_mut()
            .finish_subscribe_namespace_request(request_id, result.is_ok());
        self.emit_namespace_response(|c| &c.subscribe_namespace_response, request_id, result);
        Ok(())
    }

    /// The track alias is chosen by the session, so `track_alias` is ignored;
    /// the alias in use is returned. A rejected PUBLISH is reported through
    /// `onPublishResponse` and returns 0.
    #[wasm_bindgen(js_name = sendPublish)]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_publish(
        &self,
        request_id: u64,
        track_namespace: Vec<String>,
        track_name: String,
        _track_alias: Option<u64>,
        group_order: u8,
        content_exists: bool,
        largest_group_id: Option<u64>,
        largest_object_id: Option<u64>,
        forward: bool,
        _auth_info: String,
    ) -> Result<u64, JsValue> {
        let session = self.session()?;
        let option = PublishOption {
            group_order: group_order_from(group_order)?,
            content_exists: content_exists_from_fields(
                content_exists,
                largest_group_id,
                largest_object_id,
            ),
            forward,
        };
        let result = session
            .publisher()
            .publish(track_namespace.join("/"), track_name.clone(), option)
            .await;
        match result {
            Ok(subscription) => {
                let track_alias = subscription.track_alias();
                self.shared
                    .state
                    .borrow_mut()
                    .add_publishing_alias(TrackKey::new(track_namespace, track_name), track_alias);
                let publish_ok = PublishOk {
                    request_id,
                    forward,
                    subscriber_priority: subscription.subscriber_priority(),
                    group_order: subscription.group_order(),
                    filter_type: subscription.filter_type(),
                    delivery_timeout: None,
                };
                self.shared
                    .track_subscriptions
                    .borrow_mut()
                    .insert(track_alias, subscription);
                let message = PublishOkMessage::from(&publish_ok);
                emit(
                    &self.shared.callbacks,
                    |c| &c.publish_response,
                    &[message.into()],
                );
                Ok(track_alias)
            }
            Err(error) => {
                let message = request_error_message(request_id, &error);
                emit(
                    &self.shared.callbacks,
                    |c| &c.publish_response,
                    &[message.into()],
                );
                Ok(0)
            }
        }
    }

    #[wasm_bindgen(js_name = sendPublishOk)]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_publish_ok(
        &self,
        request_id: u64,
        subscriber_priority: u8,
        _group_order: u8,
        filter_type: u8,
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
        _delivery_timeout: Option<u64>,
        _forward: bool,
    ) -> Result<(), JsValue> {
        let session = self.session()?;
        let filter_type =
            filter_type_from_fields(filter_type, start_group, start_object, end_group)?;
        let handler = self
            .shared
            .incoming
            .borrow_mut()
            .publishes
            .remove(&request_id)
            .ok_or_else(|| js_error(format!("unknown publish request: {request_id}")))?;
        let subscription = handler
            .ok(subscriber_priority, filter_type, 0)
            .await
            .map_err(send_error)?;
        handler.accept_data_receiver().await;
        spawn_local(track_io::read_track(
            self.shared.clone(),
            session,
            subscription,
        ));
        Ok(())
    }

    #[wasm_bindgen(js_name = sendPublishError)]
    pub async fn send_publish_error(
        &self,
        request_id: u64,
        error_code: u64,
        reason_phrase: String,
    ) -> Result<(), JsValue> {
        let handler = self
            .shared
            .incoming
            .borrow_mut()
            .publishes
            .remove(&request_id)
            .ok_or_else(|| js_error(format!("unknown publish request: {request_id}")))?;
        handler
            .error(error_code, reason_phrase)
            .await
            .map_err(send_error)
    }

    #[wasm_bindgen(js_name = sendSubscribe)]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_subscribe(
        &self,
        request_id: u64,
        track_namespace: Vec<String>,
        track_name: String,
        subscriber_priority: u8,
        group_order: u8,
        filter_type: u8,
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
        _auth_info: String,
        forward: bool,
        _delivery_timeout: Option<u64>,
    ) -> Result<(), JsValue> {
        let session = self.session()?;
        let option = SubscribeOption {
            subscriber_priority,
            group_order: group_order_from(group_order)?,
            forward,
            filter_type: filter_type_from_fields(
                filter_type,
                start_group,
                start_object,
                end_group,
            )?,
        };
        let result = session
            .subscriber()
            .subscribe(track_namespace.join("/"), track_name, option)
            .await;
        match result {
            Ok(subscription) => {
                let subscribe_ok = SubscribeOk {
                    request_id,
                    track_alias: subscription.track_alias(),
                    expires: subscription.expires().unwrap_or(0),
                    group_order: subscription.group_order(),
                    content_exists: subscription.content_exists(),
                    delivery_timeout: None,
                    max_duration: None,
                };
                self.shared
                    .subscription_wire_ids
                    .borrow_mut()
                    .insert(request_id, subscription.request_id());
                self.shared
                    .subscriptions
                    .borrow_mut()
                    .insert(request_id, subscription.clone());
                let message = SubscribeOkMessage::from(&subscribe_ok);
                emit(
                    &self.shared.callbacks,
                    |c| &c.subscribe_response,
                    &[message.into()],
                );
                spawn_local(track_io::read_track(
                    self.shared.clone(),
                    session,
                    subscription,
                ));
            }
            Err(error) => {
                let message = request_error_message(request_id, &error);
                emit(
                    &self.shared.callbacks,
                    |c| &c.subscribe_response,
                    &[message.into()],
                );
            }
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = sendSubscribeForward)]
    pub async fn send_subscribe_forward(
        &self,
        _request_id: u64,
        subscription_request_id: u64,
        forward: bool,
    ) -> Result<(), JsValue> {
        let session = self.session()?;
        let subscription = self
            .shared
            .subscriptions
            .borrow()
            .get(&subscription_request_id)
            .cloned()
            .ok_or_else(|| {
                js_error(format!("no active subscription: {subscription_request_id}"))
            })?;
        session
            .subscriber()
            .update_forward(&subscription, forward)
            .await
            .map_err(anyhow_error)
    }

    #[wasm_bindgen(js_name = sendFetch)]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_fetch(
        &self,
        request_id: u64,
        track_namespace: Vec<String>,
        track_name: String,
        start_group: u64,
        start_object: u64,
        end_group: u64,
        end_object: u64,
        group_order: u8,
    ) -> Result<(), JsValue> {
        let session = self.session()?;
        let option = fetch_option(group_order)?;
        let result = session
            .subscriber()
            .fetch(
                track_namespace.join("/"),
                track_name,
                Location {
                    group_id: start_group,
                    object_id: start_object,
                },
                Location {
                    group_id: end_group,
                    object_id: end_object,
                },
                option,
            )
            .await;
        self.finish_fetch_request(session, request_id, result);
        Ok(())
    }

    #[wasm_bindgen(js_name = sendRelativeJoiningFetch)]
    pub async fn send_relative_joining_fetch(
        &self,
        request_id: u64,
        joining_request_id: u64,
        joining_start: u64,
        group_order: u8,
    ) -> Result<(), JsValue> {
        let session = self.session()?;
        let option = fetch_option(group_order)?;
        let joined = self
            .shared
            .subscriptions
            .borrow()
            .get(&joining_request_id)
            .map(Subscription::request_id)
            .ok_or_else(|| js_error(format!("no active subscription: {joining_request_id}")))?;
        let result = session
            .subscriber()
            .fetch_relative_joining(joined, joining_start, option)
            .await;
        self.finish_fetch_request(session, request_id, result);
        Ok(())
    }

    #[wasm_bindgen(js_name = isSubscribed)]
    pub fn is_subscribed(&self, request_id: u64) -> bool {
        self.shared.subscriptions.borrow().contains_key(&request_id)
    }

    #[wasm_bindgen(js_name = getTrackSubscribers)]
    pub fn get_track_subscribers(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> Vec<u64> {
        self.shared
            .state
            .borrow()
            .get_track_subscribers(track_namespace, track_name)
    }

    #[wasm_bindgen(js_name = getSubgroupState)]
    pub fn get_subgroup_state(&self, track_alias: u64) -> SubgroupState {
        self.shared
            .state
            .borrow_mut()
            .current_subgroup_state(track_alias)
    }

    #[wasm_bindgen(js_name = markSubgroupHeaderSent)]
    pub fn mark_subgroup_header_sent(&self, track_alias: u64) {
        self.shared
            .state
            .borrow_mut()
            .mark_subgroup_header_sent(track_alias);
    }

    #[wasm_bindgen(js_name = incrementSubgroupObject)]
    pub fn increment_subgroup_object(&self, track_alias: u64) {
        self.shared
            .state
            .borrow_mut()
            .increment_subgroup_object(track_alias);
    }

    #[wasm_bindgen(js_name = resetSubgroupState)]
    pub fn reset_subgroup_state(&self, track_alias: u64) {
        self.shared
            .state
            .borrow_mut()
            .reset_subgroup_state(track_alias);
    }

    #[wasm_bindgen(js_name = sendSubscribeOk)]
    pub async fn send_subscribe_ok(
        &self,
        request_id: u64,
        expires: u64,
        delivery_timeout: Option<u64>,
        max_duration: Option<u64>,
    ) -> Result<u64, JsValue> {
        let mut handler = self
            .shared
            .incoming
            .borrow_mut()
            .subscribes
            .remove(&request_id)
            .ok_or_else(|| js_error(format!("unknown subscribe request: {request_id}")))?;
        let track_alias = handler.allocate_track_alias();
        let content_exists = self
            .shared
            .state
            .borrow_mut()
            .activate_incoming_subscribe(request_id, track_alias)
            .map_err(anyhow_error)?;
        handler.delivery_timeout = delivery_timeout;
        handler.max_cache_duration = max_duration;
        handler
            .ok_with_track_alias(track_alias, expires, content_exists)
            .await
            .map_err(send_error)?;
        self.shared
            .track_subscriptions
            .borrow_mut()
            .insert(track_alias, handler.into_subscription(track_alias));
        Ok(track_alias)
    }

    #[wasm_bindgen(js_name = sendSubscribeError)]
    pub async fn send_subscribe_error(
        &self,
        request_id: u64,
        error_code: u64,
        reason_phrase: String,
    ) -> Result<(), JsValue> {
        let handler = self
            .shared
            .incoming
            .borrow_mut()
            .subscribes
            .remove(&request_id)
            .ok_or_else(|| js_error(format!("unknown subscribe request: {request_id}")))?;
        self.shared
            .state
            .borrow_mut()
            .remove_incoming_subscribe(request_id);
        handler
            .error(error_code, reason_phrase)
            .await
            .map_err(send_error)
    }

    #[wasm_bindgen(js_name = sendUnsubscribe)]
    pub async fn send_unsubscribe(&self, request_id: u64) -> Result<(), JsValue> {
        let session = self.session()?;
        let subscription = self.shared.subscriptions.borrow_mut().remove(&request_id);
        let Some(subscription) = subscription else {
            return Ok(());
        };
        session
            .subscriber()
            .unsubscribe(subscription.request_id())
            .await
            .map_err(anyhow_error)
    }

    #[wasm_bindgen(js_name = sendPublishNamespaceDone)]
    pub async fn send_publish_namespace_done(
        &self,
        track_namespace: Vec<String>,
    ) -> Result<(), JsValue> {
        let session = self.session()?;
        session
            .publisher()
            .publish_namespace_done(track_namespace.join("/"))
            .await
            .map_err(anyhow_error)
    }

    #[wasm_bindgen(js_name = sendUnsubscribeNamespace)]
    pub async fn send_unsubscribe_namespace(
        &self,
        track_namespace_prefix: Vec<String>,
    ) -> Result<(), JsValue> {
        let session = self.session()?;
        session
            .subscriber()
            .unsubscribe_namespace(track_namespace_prefix.join("/"))
            .await
            .map_err(anyhow_error)
    }

    #[wasm_bindgen(js_name = sendGoAway)]
    pub async fn send_go_away(&self, new_session_uri: String) -> Result<(), JsValue> {
        self.session()?
            .go_away(new_session_uri)
            .await
            .map_err(anyhow_error)
    }

    #[wasm_bindgen(js_name = sendMaxRequestId)]
    pub async fn send_max_request_id(&self, request_id: u64) -> Result<(), JsValue> {
        self.session()?
            .raise_max_request_id(request_id)
            .await
            .map_err(anyhow_error)
    }

    #[wasm_bindgen(js_name = sendRequestsBlocked)]
    pub async fn send_requests_blocked(&self, maximum_request_id: u64) -> Result<(), JsValue> {
        self.session()?
            .requests_blocked(maximum_request_id)
            .await
            .map_err(anyhow_error)
    }

    /// `subscription_request_id` is the id JavaScript chose for its SUBSCRIBE
    /// when this client issued it; any other value is sent as given.
    #[wasm_bindgen(js_name = sendSubscribeUpdate)]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_subscribe_update(
        &self,
        _request_id: u64,
        subscription_request_id: u64,
        start_group: u64,
        start_object: u64,
        end_group: u64,
        subscriber_priority: u8,
        forward: bool,
    ) -> Result<(), JsValue> {
        let session = self.session()?;
        let wire_request_id = self
            .shared
            .subscription_wire_ids
            .borrow()
            .get(&subscription_request_id)
            .copied()
            .unwrap_or(subscription_request_id);
        session
            .subscriber()
            .subscribe_update(
                wire_request_id,
                SubscribeUpdateOption {
                    start_location: Location {
                        group_id: start_group,
                        object_id: start_object,
                    },
                    end_group,
                    subscriber_priority,
                    forward,
                },
            )
            .await
            .map_err(anyhow_error)
    }

    #[wasm_bindgen(js_name = sendPublishDone)]
    pub async fn send_publish_done(
        &self,
        request_id: u64,
        status_code: u64,
        stream_count: u64,
        error_reason: String,
    ) -> Result<(), JsValue> {
        let session = self.session()?;
        session
            .publisher()
            .publish_done(request_id, status_code, stream_count, error_reason)
            .await
            .map_err(anyhow_error)
    }

    #[wasm_bindgen(js_name = sendFetchCancel)]
    pub async fn send_fetch_cancel(&self, request_id: u64) -> Result<(), JsValue> {
        let session = self.session()?;
        let wire_request_id = self
            .shared
            .fetch_requests
            .borrow_mut()
            .remove(&request_id)
            .unwrap_or(request_id);
        session
            .subscriber()
            .fetch_cancel(wire_request_id)
            .await
            .map_err(anyhow_error)
    }

    #[wasm_bindgen(js_name = sendPublishNamespaceCancel)]
    pub async fn send_publish_namespace_cancel(
        &self,
        track_namespace: Vec<String>,
        error_code: u64,
        error_reason: String,
    ) -> Result<(), JsValue> {
        self.session()?
            .publisher()
            .publish_namespace_cancel(track_namespace.join("/"), error_code, error_reason)
            .await
            .map_err(anyhow_error)
    }

    #[wasm_bindgen(js_name = sendTrackStatus)]
    pub async fn send_track_status(
        &self,
        request_id: u64,
        track_namespace: Vec<String>,
        track_name: String,
        auth_info: String,
    ) -> Result<(), JsValue> {
        let session = self.session()?;
        let result = session
            .subscriber()
            .track_status(
                track_namespace.join("/"),
                track_name,
                authorization_tokens(&auth_info),
            )
            .await;
        let message: JsValue = match result {
            Ok(track_status_ok) => SubscribeOkMessage::from(&TrackStatusOk {
                request_id,
                ..track_status_ok
            })
            .into(),
            Err(error) => request_error_message(request_id, &error).into(),
        };
        emit(
            &self.shared.callbacks,
            |c| &c.track_status_response,
            &[message],
        );
        Ok(())
    }

    #[wasm_bindgen(js_name = sendTrackStatusOk)]
    pub async fn send_track_status_ok(&self, request_id: u64) -> Result<(), JsValue> {
        let handler = self
            .shared
            .incoming
            .borrow_mut()
            .track_statuses
            .remove(&request_id)
            .ok_or_else(|| js_error(format!("unknown track status request: {request_id}")))?;
        let track_status_ok = self
            .shared
            .state
            .borrow_mut()
            .answer_incoming_track_status(request_id)
            .map_err(anyhow_error)?;
        handler
            .ok(track_status_ok.expires, track_status_ok.content_exists)
            .await
            .map_err(send_error)
    }

    #[wasm_bindgen(js_name = sendTrackStatusError)]
    pub async fn send_track_status_error(
        &self,
        request_id: u64,
        error_code: u64,
        reason_phrase: String,
    ) -> Result<(), JsValue> {
        let handler = self
            .shared
            .incoming
            .borrow_mut()
            .track_statuses
            .remove(&request_id)
            .ok_or_else(|| js_error(format!("unknown track status request: {request_id}")))?;
        self.shared
            .state
            .borrow_mut()
            .reject_incoming_track_status(request_id)
            .map_err(anyhow_error)?;
        handler
            .error(error_code, reason_phrase)
            .await
            .map_err(send_error)
    }

    #[wasm_bindgen(js_name = sendFetchOk)]
    pub async fn send_fetch_ok(&self, request_id: u64) -> Result<(), JsValue> {
        let handler = self
            .shared
            .incoming
            .borrow_mut()
            .fetches
            .remove(&request_id)
            .ok_or_else(|| js_error(format!("unknown fetch request: {request_id}")))?;
        let fetch_ok = self
            .shared
            .state
            .borrow()
            .answer_incoming_fetch(request_id)
            .map_err(anyhow_error)?;
        handler
            .ok(fetch_ok.end_of_track, fetch_ok.end_location)
            .await
            .map_err(send_error)
    }

    #[wasm_bindgen(js_name = sendFetchError)]
    pub async fn send_fetch_error(
        &self,
        request_id: u64,
        error_code: u64,
        reason_phrase: String,
    ) -> Result<(), JsValue> {
        self.shared
            .state
            .borrow_mut()
            .remove_incoming_fetch(request_id)
            .map_err(anyhow_error)?;
        self.shared.reset_fetch_sender(request_id);
        let handler = self
            .shared
            .incoming
            .borrow_mut()
            .fetches
            .remove(&request_id);
        match handler {
            Some(handler) => handler
                .error(error_code, reason_phrase)
                .await
                .map_err(send_error),
            None => Ok(()),
        }
    }

    #[wasm_bindgen(js_name = sendFetchObject)]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_fetch_object(
        &self,
        request_id: u64,
        group_id: u64,
        subgroup_id: u64,
        object_id: u64,
        publisher_priority: u8,
        object_status: Option<u8>,
        object_payload: Vec<u8>,
        loc_header: JsValue,
    ) -> Result<(), JsValue> {
        let fetch_object = match object_status {
            Some(status) => FetchObject::Status(object_status_from(status)?),
            None => FetchObject::Payload(Bytes::from(object_payload)),
        };
        let field = FetchObjectField::new(
            group_id,
            subgroup_id,
            object_id,
            publisher_priority,
            crate::loc::parse_loc_header(loc_header)?,
            fetch_object,
        );
        let sender = self.fetch_sender(request_id).await?;
        let sender = sender.lock().await;
        sender.send(field).await.map_err(anyhow_error)
    }

    #[wasm_bindgen(js_name = finishFetch)]
    pub async fn finish_fetch(&self, request_id: u64) -> Result<(), JsValue> {
        let sender = self.fetch_sender(request_id).await?;
        self.shared
            .state
            .borrow_mut()
            .remove_incoming_fetch(request_id)
            .map_err(anyhow_error)?;
        self.shared.fetch_senders.borrow_mut().remove(&request_id);
        let sender = sender.lock().await;
        sender.close().await.map_err(anyhow_error)
    }

    #[wasm_bindgen(js_name = sendObjectDatagram)]
    pub async fn send_object_datagram(
        &self,
        track_alias: u64,
        group_id: u64,
        object_id: u64,
        publisher_priority: u8,
        object_payload: Vec<u8>,
        loc_header: JsValue,
    ) -> Result<(), JsValue> {
        let payload = ObjectDatagramPayload::Payload(Bytes::from(object_payload));
        self.send_datagram_field(
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            loc_header,
            payload,
        )
        .await
    }

    #[wasm_bindgen(js_name = sendObjectDatagramStatus)]
    pub async fn send_object_datagram_status(
        &self,
        track_alias: u64,
        group_id: u64,
        object_id: u64,
        publisher_priority: u8,
        object_status: u8,
        loc_header: JsValue,
    ) -> Result<(), JsValue> {
        let payload = ObjectDatagramPayload::Status(object_status_from(object_status)?);
        self.send_datagram_field(
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            loc_header,
            payload,
        )
        .await
    }

    #[wasm_bindgen(js_name = sendSubgroupHeader)]
    pub async fn send_subgroup_header(
        &self,
        track_alias: u64,
        group_id: u64,
        subgroup_id: u64,
        publisher_priority: u8,
    ) -> Result<(), JsValue> {
        let writer_key = (track_alias, group_id, subgroup_id);
        if self
            .shared
            .stream_senders
            .borrow()
            .contains_key(&writer_key)
        {
            return Ok(());
        }
        let session = self.session()?;
        let subscription = self.track_subscription(track_alias)?;
        let sender = session
            .publisher()
            .create_stream(&subscription)
            .next()
            .await
            .map_err(anyhow_error)?;
        let header = sender.create_header(
            group_id,
            SubgroupId::Value(subgroup_id),
            publisher_priority,
            true,
            true,
        );
        let sender = sender.send_header(header).await.map_err(anyhow_error)?;
        self.shared
            .stream_senders
            .borrow_mut()
            .insert(writer_key, Rc::new(Mutex::new(sender)));
        self.shared
            .stream_object_numbers
            .borrow_mut()
            .remove(&writer_key);
        Ok(())
    }

    #[wasm_bindgen(js_name = sendSubgroupObject)]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_subgroup_object(
        &self,
        track_alias: u64,
        group_id: u64,
        subgroup_id: u64,
        object_number: u64,
        object_status: Option<u8>,
        object_payload: Vec<u8>,
        loc_header: JsValue,
    ) -> Result<(), JsValue> {
        let writer_key = (track_alias, group_id, subgroup_id);
        let sender = self
            .shared
            .stream_senders
            .borrow()
            .get(&writer_key)
            .cloned()
            .ok_or_else(|| js_error("subgroup writer is None"))?;
        let extension_headers = crate::loc::parse_loc_header(loc_header)?;
        let object_id_delta = {
            let previous = self
                .shared
                .stream_object_numbers
                .borrow()
                .get(&writer_key)
                .copied();
            match previous {
                Some(previous_object_number) => previous_object_number
                    .checked_add(1)
                    .and_then(|next_object_number| object_number.checked_sub(next_object_number)),
                None => Some(object_number),
            }
            .ok_or_else(|| {
                js_error("object number must increase monotonically within a subgroup stream")
            })?
        };
        let status = object_status.map(object_status_from).transpose()?;
        let subgroup_object = match status {
            Some(status) => SubgroupObject::new_status(status as u64),
            None => SubgroupObject::new_payload(Bytes::from(object_payload)),
        };
        let mut sender = sender.lock().await;
        let field = sender.create_object_field(object_id_delta, extension_headers, subgroup_object);
        sender.send(field).await.map_err(anyhow_error)?;
        self.shared.state.borrow_mut().record_published_object(
            track_alias,
            Location {
                group_id,
                object_id: object_number,
            },
        );
        self.shared
            .stream_object_numbers
            .borrow_mut()
            .insert(writer_key, object_number);
        if matches!(
            status,
            Some(ObjectStatus::EndOfGroup | ObjectStatus::EndOfTrack)
        ) {
            let _ = sender.close().await;
            self.shared.stream_senders.borrow_mut().remove(&writer_key);
            self.shared
                .stream_object_numbers
                .borrow_mut()
                .remove(&writer_key);
        }
        Ok(())
    }
}

impl MOQTClient {
    fn session(&self) -> Result<Rc<Session>, JsValue> {
        self.shared
            .session
            .borrow()
            .clone()
            .ok_or_else(|| js_error("MOQT client is not connected"))
    }

    fn track_subscription(&self, track_alias: u64) -> Result<Subscription, JsValue> {
        self.shared
            .track_subscriptions
            .borrow()
            .get(&track_alias)
            .cloned()
            .ok_or_else(|| js_error(format!("unknown track alias: {track_alias}")))
    }

    fn emit_namespace_response(
        &self,
        select: CallbackSelector,
        request_id: u64,
        result: anyhow::Result<()>,
    ) {
        let message: JsValue = match result {
            Ok(()) => NamespaceOkMessage::from(&NamespaceOk { request_id }).into(),
            Err(error) => request_error_message(request_id, &error).into(),
        };
        emit(&self.shared.callbacks, select, &[message]);
    }

    fn finish_fetch_request(
        &self,
        session: Rc<Session>,
        request_id: u64,
        result: anyhow::Result<FetchHandle>,
    ) {
        match result {
            Ok(fetch_handle) => {
                let fetch_ok = FetchOk {
                    request_id,
                    group_order: fetch_handle.group_order,
                    end_of_track: fetch_handle.end_of_track,
                    end_location: fetch_handle.end_location,
                    max_cache_duration: None,
                };
                self.shared
                    .fetch_requests
                    .borrow_mut()
                    .insert(request_id, fetch_handle.request_id);
                let message = FetchOkMessage::from(&fetch_ok);
                emit(
                    &self.shared.callbacks,
                    |c| &c.fetch_response,
                    &[message.into()],
                );
                spawn_local(track_io::read_fetch(
                    self.shared.clone(),
                    session,
                    request_id,
                    fetch_handle,
                ));
            }
            Err(error) => {
                let message = request_error_message(request_id, &error);
                emit(
                    &self.shared.callbacks,
                    |c| &c.fetch_response,
                    &[message.into()],
                );
            }
        }
    }

    async fn fetch_sender(&self, request_id: u64) -> Result<Rc<Mutex<FetchDataSender>>, JsValue> {
        if !self
            .shared
            .state
            .borrow()
            .contains_incoming_fetch(request_id)
        {
            return Err(js_error(format!("unknown fetch request: {request_id}")));
        }
        let existing = self.shared.fetch_senders.borrow().get(&request_id).cloned();
        if let Some(sender) = existing {
            return Ok(sender);
        }
        let session = self.session()?;
        let sender = session
            .publisher()
            .create_fetch_stream(request_id)
            .await
            .map_err(anyhow_error)?;
        if !self
            .shared
            .state
            .borrow()
            .contains_incoming_fetch(request_id)
        {
            let _ = sender.reset(0).await;
            return Err(js_error(format!(
                "fetch request {request_id} was cancelled"
            )));
        }
        let sender = Rc::new(Mutex::new(sender));
        self.shared
            .fetch_senders
            .borrow_mut()
            .insert(request_id, sender.clone());
        Ok(sender)
    }

    async fn send_datagram_field(
        &self,
        track_alias: u64,
        group_id: u64,
        object_id: u64,
        publisher_priority: u8,
        loc_header: JsValue,
        payload: ObjectDatagramPayload,
    ) -> Result<(), JsValue> {
        let session = self.session()?;
        let subscription = self.track_subscription(track_alias)?;
        let extension_headers = crate::loc::parse_loc_header(loc_header)?;
        let field = DatagramField {
            object_id: Some(object_id),
            publisher_priority,
            extension_headers: (!extension_headers.key_value_pairs.is_empty())
                .then_some(extension_headers),
            end_of_group: false,
            payload,
        };
        session
            .publisher()
            .create_datagram(&subscription)
            .send(ObjectDatagram::new(track_alias, group_id, field))
            .await
            .map_err(anyhow_error)?;
        self.shared.state.borrow_mut().record_published_object(
            track_alias,
            Location {
                group_id,
                object_id,
            },
        );
        Ok(())
    }
}

fn anyhow_error(error: anyhow::Error) -> JsValue {
    js_error(format!("{error:#}"))
}

fn send_error(error: TransportSendError) -> JsValue {
    js_error(error.to_string())
}

/// Reports a failed request to JavaScript under the request id it chose.
/// A `RequestError` keeps the peer's code and reason; a timeout maps to
/// TIMEOUT and anything else to INTERNAL_ERROR (draft-14 §13.1).
fn request_error_message(request_id: u64, error: &anyhow::Error) -> RequestErrorMessage {
    let request_error = match error.downcast_ref::<RequestError>() {
        Some(request_error) => RequestError {
            request_id,
            error_code: request_error.error_code,
            reason_phrase: request_error.reason_phrase.clone(),
        },
        None => RequestError {
            request_id,
            error_code: if error.downcast_ref::<RequestTimeoutError>().is_some() {
                ERROR_TIMEOUT
            } else {
                ERROR_INTERNAL
            },
            reason_phrase: format!("{error:#}"),
        },
    };
    RequestErrorMessage::from(&request_error)
}

fn authorization_tokens(auth_info: &str) -> Vec<AuthorizationToken> {
    if auth_info.trim().is_empty() {
        return vec![];
    }
    vec![AuthorizationToken::use_value_utf8(auth_info)]
}

fn content_exists_from_fields(
    content_exists: bool,
    largest_group_id: Option<u64>,
    largest_object_id: Option<u64>,
) -> ContentExists {
    if content_exists {
        ContentExists::True {
            location: Location {
                group_id: largest_group_id.unwrap_or(0),
                object_id: largest_object_id.unwrap_or(0),
            },
        }
    } else {
        ContentExists::False
    }
}

fn group_order_from(value: u8) -> Result<GroupOrder, JsValue> {
    GroupOrder::try_from(value).map_err(|_| js_error("invalid group order"))
}

fn object_status_from(value: u8) -> Result<ObjectStatus, JsValue> {
    ObjectStatus::try_from(value).map_err(|_| js_error("invalid object status"))
}

/// The previous client sent FETCH with Subscriber Priority 0.
fn fetch_option(group_order: u8) -> Result<FetchOption, JsValue> {
    Ok(FetchOption {
        subscriber_priority: 0,
        group_order: group_order_from(group_order)?,
    })
}

fn filter_type_from_fields(
    filter_type: u8,
    start_group: Option<u64>,
    start_object: Option<u64>,
    end_group: Option<u64>,
) -> Result<FilterType, JsValue> {
    match filter_type {
        1 => Ok(FilterType::NextGroupStart),
        2 => Ok(FilterType::LargestObject),
        3 => Ok(FilterType::AbsoluteStart {
            location: Location {
                group_id: start_group.unwrap_or(0),
                object_id: start_object.unwrap_or(0),
            },
        }),
        4 => Ok(FilterType::AbsoluteRange {
            location: Location {
                group_id: start_group.unwrap_or(0),
                object_id: start_object.unwrap_or(0),
            },
            end_group: end_group.unwrap_or(0),
        }),
        _ => Err(js_error("invalid filter type")),
    }
}
