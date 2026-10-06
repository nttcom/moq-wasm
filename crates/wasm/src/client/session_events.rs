use std::rc::Rc;

use moqt::{
    FetchHandler, PublishHandler, PublishNamespaceHandler, Session, SessionEvent, SubscribeHandler,
    TrackStatusHandler,
    wire::{Publish, PublishNamespace, PublishNamespaceDone, Subscribe, TrackStatus},
};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;

use super::{
    ClientShared,
    callbacks::{emit, has_callback},
    console_log,
};
use crate::{
    messages::{
        FetchMessage, PublishMessage, PublishNamespaceDoneMessage, PublishNamespaceMessage,
        SubscribeMessage,
    },
    request_rejection::RequestRejection,
};

/// Turns the session's events into the JavaScript callbacks and parks every
/// request handler until JavaScript answers it. Returns when the session ends.
pub(crate) async fn run_session_events(shared: Rc<ClientShared>, session: Rc<Session>) {
    while let Ok(event) = session.receive_event().await {
        match event {
            SessionEvent::PublishNamespace(handler) => on_publish_namespace(&shared, handler),
            SessionEvent::PublishNamespaceDone(handler) => {
                let namespace = handler
                    .track_namespace()
                    .split('/')
                    .map(str::to_string)
                    .collect();
                let message =
                    PublishNamespaceDoneMessage::from(&PublishNamespaceDone::new(namespace));
                emit(
                    &shared.callbacks,
                    |c| &c.publish_namespace_done,
                    &[message.into()],
                );
            }
            SessionEvent::Publish(handler) => on_publish(&shared, handler),
            SessionEvent::Subscribe(handler) => on_subscribe(&shared, handler),
            SessionEvent::Unsubscribe(handler) => {
                let request_id = handler.subscribe_id();
                shared
                    .state
                    .borrow_mut()
                    .remove_incoming_subscribe(request_id);
                emit(
                    &shared.callbacks,
                    |c| &c.incoming_unsubscribe,
                    &[js_sys::BigInt::from(request_id).into()],
                );
            }
            SessionEvent::TrackStatus(handler) => on_track_status(&shared, handler),
            SessionEvent::Fetch(handler) => on_fetch(&shared, handler),
            SessionEvent::FetchCancel(handler) => on_fetch_cancel(&shared, handler.request_id()),
            SessionEvent::Disconnected() | SessionEvent::ProtocolViolation() => break,
            other => console_log(&format!("Unhandled session event: {other:?}")),
        }
    }
    shared.on_disconnected();
}

fn on_publish_namespace(shared: &ClientShared, handler: PublishNamespaceHandler) {
    let publish_namespace = PublishNamespace::new(
        handler.request_id(),
        handler.track_namespace_tuple.clone(),
        vec![],
    );
    shared
        .incoming
        .borrow_mut()
        .publish_namespaces
        .insert(publish_namespace.request_id, handler);
    let message = PublishNamespaceMessage::from(&publish_namespace);
    emit(
        &shared.callbacks,
        |c| &c.publish_namespace,
        &[message.into()],
    );
}

fn on_publish(shared: &ClientShared, handler: PublishHandler) {
    let publish = Publish {
        request_id: handler.request_id,
        track_namespace_tuple: handler.track_namespace_tuple.clone(),
        track_name: handler.track_name.clone(),
        track_alias: handler.track_alias,
        group_order: handler.group_order,
        content_exists: handler.content_exists,
        forward: handler.forward,
        authorization_tokens: vec![],
        delivery_timeout: handler.delivery_timeout,
        max_duration: handler.max_cache_duration,
    };
    shared
        .incoming
        .borrow_mut()
        .publishes
        .insert(publish.request_id, handler);
    let message = PublishMessage::from(&publish);
    emit(&shared.callbacks, |c| &c.publish, &[message.into()]);
}

fn on_subscribe(shared: &ClientShared, handler: SubscribeHandler) {
    let subscribe = Subscribe {
        request_id: handler.request_id(),
        track_namespace: handler.track_namespace_tuple.clone(),
        track_name: handler.track_name.clone(),
        subscriber_priority: handler.subscriber_priority,
        group_order: handler.group_order,
        forward: handler.forward,
        filter_type: handler.filter_type,
        authorization_tokens: vec![],
        delivery_timeout: handler.delivery_timeout,
    };
    let validation_code = {
        let mut state = shared.state.borrow_mut();
        let validation_code = state.validate_incoming_subscribe(&subscribe);
        if validation_code == 0 {
            state.register_incoming_subscribe(&subscribe);
        }
        validation_code
    };
    shared
        .incoming
        .borrow_mut()
        .subscribes
        .insert(subscribe.request_id, handler);
    let message = SubscribeMessage::from(&subscribe);
    emit(
        &shared.callbacks,
        |c| &c.subscribe,
        &[
            message.into(),
            JsValue::from_bool(validation_code == 0),
            JsValue::from_f64(validation_code as f64),
        ],
    );
}

fn on_track_status(shared: &ClientShared, handler: TrackStatusHandler) {
    if !has_callback(&shared.callbacks, |c| &c.track_status) {
        reject_track_status(handler, RequestRejection::NotSupported);
        return;
    }
    let track_status = TrackStatus {
        request_id: handler.request_id(),
        track_namespace: handler.track_namespace_tuple().to_vec(),
        track_name: handler.track_name().to_string(),
        subscriber_priority: handler.subscriber_priority(),
        group_order: handler.group_order(),
        forward: handler.forward(),
        filter_type: handler.filter_type(),
        authorization_tokens: handler.authorization_tokens().to_vec(),
        delivery_timeout: None,
    };
    let accepted = shared
        .state
        .borrow_mut()
        .accept_incoming_track_status(&track_status);
    if let Err(rejection) = accepted {
        reject_track_status(handler, rejection);
        return;
    }
    shared
        .incoming
        .borrow_mut()
        .track_statuses
        .insert(track_status.request_id, handler);
    let message = SubscribeMessage::from(&track_status);
    emit(&shared.callbacks, |c| &c.track_status, &[message.into()]);
}

fn reject_track_status(handler: TrackStatusHandler, rejection: RequestRejection) {
    let (code, reason) = rejection.code_and_reason();
    spawn_local(async move {
        let _ = handler.error(code, reason.to_string()).await;
    });
}

fn on_fetch(shared: &ClientShared, handler: FetchHandler) {
    if !has_callback(&shared.callbacks, |c| &c.fetch) {
        reject_fetch(handler, RequestRejection::NotSupported);
        return;
    }
    let accepted = shared
        .state
        .borrow_mut()
        .accept_incoming_fetch(&handler.fetch);
    match accepted {
        Ok(request) => {
            let message = FetchMessage::new(handler.request_id, &request);
            shared
                .incoming
                .borrow_mut()
                .fetches
                .insert(handler.request_id, handler);
            emit(&shared.callbacks, |c| &c.fetch, &[message.into()]);
        }
        Err(rejection) => reject_fetch(handler, rejection),
    }
}

fn reject_fetch(handler: FetchHandler, rejection: RequestRejection) {
    let (code, reason) = rejection.code_and_reason();
    spawn_local(async move {
        let _ = handler.error(code, reason.to_string()).await;
    });
}

fn on_fetch_cancel(shared: &ClientShared, request_id: u64) {
    if shared
        .state
        .borrow_mut()
        .remove_incoming_fetch(request_id)
        .is_err()
    {
        return;
    }
    shared.incoming.borrow_mut().fetches.remove(&request_id);
    shared.reset_fetch_sender(request_id);
    emit(
        &shared.callbacks,
        |c| &c.fetch_cancel,
        &[js_sys::BigInt::from(request_id).into()],
    );
}
