use std::sync::{Arc, Weak};

use tracing::{Instrument, Span};

use crate::{
    SessionEvent, TransportProtocol,
    modules::moqt::{
        control_plane::{
            constants::TerminationErrorCode,
            enums::ResponseMessage,
            handler::{
                fetch_cancel_handler::FetchCancelHandler, fetch_handler::FetchHandler,
                go_away_handler::GoAwayHandler, max_request_id_handler::MaxRequestIdHandler,
                publish_done_handler::PublishDoneHandler, publish_handler::PublishHandler,
                publish_namespace_cancel_handler::PublishNamespaceCancelHandler,
                publish_namespace_done_handler::PublishNamespaceDoneHandler,
                publish_namespace_handler::PublishNamespaceHandler,
                requests_blocked_handler::RequestsBlockedHandler,
                subscribe_handler::SubscribeHandler,
                subscribe_namespace_handler::SubscribeNamespaceHandler,
                subscribe_update_handler::SubscribeUpdateHandler,
                track_status_handler::TrackStatusHandler, unsubscribe_handler::UnsubscribeHandler,
                unsubscribe_namespace_handler::UnsubscribeNamespaceHandler,
            },
        },
        data_plane::stream::{
            received_message::ReceivedMessage, stream_receiver::BiStreamReceiver,
        },
        domains::session_context::{InflightRequest, SessionContext},
    },
};

enum DepacketizeResult<T: TransportProtocol> {
    SessionEvent(SessionEvent<T>),
    ResponseMessage(u64, ResponseMessage),
    /// The session was closed while resolving the message; the caller must
    /// stop reading instead of emitting another event.
    SessionClosed,
}

pub(crate) struct ControlMessageReceiveTask;

impl ControlMessageReceiveTask {
    pub(crate) fn run<T: TransportProtocol>(
        mut receive_stream: BiStreamReceiver<T>,
        session_context: Weak<SessionContext<T>>,
        receiver_span: Span,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Control Message Receiver")
            .spawn(
                async move {
                    loop {
                        if let Some(session) = session_context.upgrade() {
                            let received_message = match receive_stream.receive().await {
                                Ok(Some(received_message)) => {
                                    tracing::info!(message = ?received_message, "Message received");
                                    received_message
                                }
                                Ok(None) => {
                                    tracing::info!("Stream ended.");
                                    break;
                                }
                                Err(error) => {
                                    tracing::info!(%error, "Stream closed.");
                                    break;
                                }
                            };

                            match Self::resolve_message(session.clone(), received_message) {
                                DepacketizeResult::SessionEvent(event) => {
                                    if let Err(error) = session.event_sender.send(event) {
                                        tracing::error!("failed to send message: {:?}", error);
                                    }
                                }
                                DepacketizeResult::SessionClosed => break,
                                DepacketizeResult::ResponseMessage(request_id, message) => {
                                    let inflight_request = session
                                        .sender_map
                                        .lock()
                                        .expect("sender_map poisoned")
                                        .remove(&request_id);
                                    match inflight_request {
                                        Some(InflightRequest::Waiting {
                                            sender,
                                            on_late_response,
                                        }) => {
                                            // The caller can stop waiting between response
                                            // arrival and this send; fall back to the
                                            // late-response withdrawal in that case.
                                            if let Err(message) = sender.send(message) {
                                                session
                                                    .handle_late_response(
                                                        request_id,
                                                        on_late_response,
                                                        message,
                                                    )
                                                    .await;
                                            }
                                        }
                                        Some(InflightRequest::Abandoned(action)) => {
                                            session
                                                .handle_late_response(request_id, action, message)
                                                .await;
                                        }
                                        None => {
                                            tracing::error!(
                                                request_id,
                                                "Protocol violation: response for unknown or already-completed Request ID; closing session"
                                            );
                                            session.close_with_error(
                                                TerminationErrorCode::ProtocolViolation,
                                                "received response for unknown or already-completed Request ID",
                                            );
                                            break;
                                        }
                                    }
                                }
                            }
                        } else {
                            tracing::error!("Session dropped.");
                            break;
                        }
                    }
                }
                .instrument(receiver_span),
            )
            .unwrap()
    }

    fn resolve_message<T: TransportProtocol>(
        session: Arc<SessionContext<T>>,
        received_message: ReceivedMessage,
    ) -> DepacketizeResult<T> {
        tracing::debug!(message = ?received_message, "Event: message_type");
        match received_message {
            ReceivedMessage::Subscribe(subscribe) => {
                tracing::debug!("Event: Subscribe");
                let subscribe_handler = SubscribeHandler::new(session.clone(), subscribe);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::Subscribe(subscribe_handler))
            }
            ReceivedMessage::Unsubscribe(unsubscribe) => {
                tracing::debug!("Event: Unsubscribe");
                let unsubscribe_handler = UnsubscribeHandler::new(session.clone(), unsubscribe);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::Unsubscribe(unsubscribe_handler))
            }
            ReceivedMessage::SubscribeOk(subscribe_ok) => {
                tracing::debug!("Event: Subscribe ok");
                let request_id = subscribe_ok.request_id;
                let response = ResponseMessage::SubscribeOk(subscribe_ok);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::SubscribeError(subscribe_error) => {
                tracing::debug!("Event: Subscribe error");
                let response = ResponseMessage::SubscribeError(
                    subscribe_error.request_id,
                    subscribe_error.error_code,
                    subscribe_error.reason_phrase,
                );
                DepacketizeResult::ResponseMessage(subscribe_error.request_id, response)
            }
            ReceivedMessage::Publish(publish) => {
                tracing::debug!("Event: Publish");
                let publish_handler = PublishHandler::new(session.clone(), publish);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::Publish(publish_handler))
            }
            ReceivedMessage::PublishOk(publish_ok) => {
                tracing::debug!("Event: Publish ok");
                let request_id = publish_ok.request_id;
                let response = ResponseMessage::PublishOk(publish_ok);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::PublishError(publish_error) => {
                tracing::debug!("Event: Publish error");
                let request_id = publish_error.request_id;
                let error_code = publish_error.error_code;
                let reason_phrase = publish_error.reason_phrase.clone();
                let response = ResponseMessage::PublishError(request_id, error_code, reason_phrase);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::PublishNamespace(publish_namespace) => {
                tracing::debug!("Event: Publish namespace");
                let publish_namespace_handler =
                    PublishNamespaceHandler::new(session.clone(), publish_namespace);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::PublishNamespace(
                    publish_namespace_handler,
                ))
            }
            ReceivedMessage::PublishNamespaceDone(publish_namespace_done) => {
                tracing::debug!("Event: Publish namespace done");
                let publish_namespace_done_handler =
                    PublishNamespaceDoneHandler::new(publish_namespace_done);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::PublishNamespaceDone(
                    publish_namespace_done_handler,
                ))
            }
            ReceivedMessage::PublishNamespaceOk(publish_namespace_ok) => {
                tracing::debug!("Event: Publish namespace ok");
                let request_id = publish_namespace_ok.request_id;
                let response = ResponseMessage::PublishNamespaceOk(request_id);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::PublishNamespaceError(publish_namespace_error) => {
                tracing::debug!("Event: Publish namespace error");
                let request_id = publish_namespace_error.request_id;
                let error_code = publish_namespace_error.error_code;
                let reason_phrase = publish_namespace_error.reason_phrase.clone();
                let response =
                    ResponseMessage::PublishNamespaceError(request_id, error_code, reason_phrase);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::SubscribeNamespace(subscribe_namespace) => {
                tracing::debug!("Event: Subscribe namespace");
                let subscribe_namespace_handler =
                    SubscribeNamespaceHandler::new(session.clone(), subscribe_namespace);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::SubscribeNameSpace(
                    subscribe_namespace_handler,
                ))
            }
            ReceivedMessage::SubscribeNamespaceOk(subscribe_namespace_ok) => {
                tracing::debug!("Event: Subscribe namespace ok");
                let request_id = subscribe_namespace_ok.request_id;
                let response = ResponseMessage::SubscribeNameSpaceOk(request_id);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::SubscribeNamespaceError(subscribe_namespace_error) => {
                tracing::debug!("Event: Subscribe namespace error");
                let request_id = subscribe_namespace_error.request_id;
                let error_code = subscribe_namespace_error.error_code;
                let reason_phrase = subscribe_namespace_error.reason_phrase.clone();
                let response =
                    ResponseMessage::SubscribeNameSpaceError(request_id, error_code, reason_phrase);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::UnsubscribeNamespace(unsubscribe_namespace) => {
                tracing::debug!("Event: Unsubscribe namespace");
                let unsubscribe_namespace_handler =
                    UnsubscribeNamespaceHandler::new(unsubscribe_namespace);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::UnsubscribeNamespace(
                    unsubscribe_namespace_handler,
                ))
            }
            ReceivedMessage::Fetch(fetch) => {
                tracing::debug!("Event: Fetch");
                let fetch_handler = FetchHandler::new(session.clone(), fetch);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::Fetch(fetch_handler))
            }
            ReceivedMessage::FetchOk(fetch_ok) => {
                tracing::debug!("Event: Fetch ok");
                let request_id = fetch_ok.request_id;
                let response = ResponseMessage::FetchOk(fetch_ok);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::FetchError(fetch_error) => {
                tracing::debug!("Event: Fetch error");
                let response = ResponseMessage::FetchError(
                    fetch_error.request_id,
                    fetch_error.error_code,
                    fetch_error.reason_phrase,
                );
                DepacketizeResult::ResponseMessage(fetch_error.request_id, response)
            }
            ReceivedMessage::SubscribeUpdate(subscribe_update) => {
                tracing::debug!("Event: Subscribe update");
                let subscribe_update_handler = SubscribeUpdateHandler::new(subscribe_update);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::SubscribeUpdate(
                    subscribe_update_handler,
                ))
            }
            ReceivedMessage::PublishDone(publish_done) => {
                tracing::debug!("Event: Publish done");
                let publish_done_handler = PublishDoneHandler::new(publish_done);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::PublishDone(
                    publish_done_handler,
                ))
            }
            ReceivedMessage::FetchCancel(fetch_cancel) => {
                tracing::debug!("Event: Fetch cancel");
                let fetch_cancel_handler = FetchCancelHandler::new(fetch_cancel);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::FetchCancel(
                    fetch_cancel_handler,
                ))
            }
            ReceivedMessage::PublishNamespaceCancel(publish_namespace_cancel) => {
                tracing::debug!("Event: Publish namespace cancel");
                let publish_namespace_cancel_handler =
                    PublishNamespaceCancelHandler::new(publish_namespace_cancel);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::PublishNamespaceCancel(
                    publish_namespace_cancel_handler,
                ))
            }
            ReceivedMessage::GoAway(go_away) => {
                tracing::debug!("Event: Go away");
                let go_away_handler = GoAwayHandler::new(go_away);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::GoAway(go_away_handler))
            }
            ReceivedMessage::MaxRequestId(max_request_id) => {
                tracing::debug!("Event: Max request id");
                let max_request_id_handler = MaxRequestIdHandler::new(max_request_id);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::MaxRequestId(
                    max_request_id_handler,
                ))
            }
            ReceivedMessage::RequestsBlocked(requests_blocked) => {
                tracing::debug!("Event: Requests blocked");
                let requests_blocked_handler = RequestsBlockedHandler::new(requests_blocked);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::RequestsBlocked(
                    requests_blocked_handler,
                ))
            }
            ReceivedMessage::TrackStatus(track_status) => {
                tracing::debug!("Event: Track status");
                let track_status_handler = TrackStatusHandler::new(session.clone(), track_status);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::TrackStatus(
                    track_status_handler,
                ))
            }
            // Sending TRACK_STATUS is not implemented, so any response to one
            // is unsolicited. Route these through `ResponseMessage` once a
            // TRACK_STATUS request API exists.
            ReceivedMessage::TrackStatusOk(track_status_ok) => {
                tracing::error!(
                    request_id = track_status_ok.request_id,
                    "Protocol violation: TRACK_STATUS_OK for a request that was never sent"
                );
                session.close_with_error(
                    TerminationErrorCode::ProtocolViolation,
                    "TRACK_STATUS_OK for a request that was never sent",
                );
                DepacketizeResult::SessionClosed
            }
            ReceivedMessage::TrackStatusError(track_status_error) => {
                tracing::error!(
                    request_id = track_status_error.request_id,
                    error_code = track_status_error.error_code,
                    reason_phrase = %track_status_error.reason_phrase,
                    "Protocol violation: TRACK_STATUS_ERROR for a request that was never sent"
                );
                session.close_with_error(
                    TerminationErrorCode::ProtocolViolation,
                    "TRACK_STATUS_ERROR for a request that was never sent",
                );
                DepacketizeResult::SessionClosed
            }
            // SETUP is exchanged before this task starts, so a later one means
            // the peer's state machine diverged from ours.
            ReceivedMessage::ClientSetup(_) | ReceivedMessage::ServerSetup(_) => {
                tracing::error!(
                    "Protocol violation: SETUP received after the session was established"
                );
                session.close_with_error(
                    TerminationErrorCode::ProtocolViolation,
                    "SETUP received after the session was established",
                );
                DepacketizeResult::SessionClosed
            }
            ReceivedMessage::FatalError() => {
                tracing::error!("Protocol violation: malformed control message payload");
                session.close_with_error(
                    TerminationErrorCode::ProtocolViolation,
                    "malformed control message payload",
                );
                DepacketizeResult::SessionClosed
            }
        }
    }
}
