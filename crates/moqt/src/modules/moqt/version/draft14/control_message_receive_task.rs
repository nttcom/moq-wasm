use std::sync::{Arc, Weak};

use tracing::{Instrument, Span};

use crate::{
    SessionEvent,
    modules::executor::{self, JoinHandle},
    modules::moqt::{
        control_plane::{
            constants::TerminationErrorCode,
            enums::{RequestId, ResponseMessage},
            error_codes::{PublishDoneCode, RequestKind},
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
        domains::{connection::Connection, session_context::SessionContext},
        version::draft14::{ControlChannel, InflightRequest, error_codes, handle_late_response},
    },
};

enum DepacketizeResult {
    SessionEvent(SessionEvent),
    ResponseMessage(u64, ResponseMessage),
    SessionClosed,
}

pub(crate) struct ControlMessageReceiveTask;

impl ControlMessageReceiveTask {
    pub(crate) fn run(
        mut receive_stream: BiStreamReceiver,
        session_context: Weak<SessionContext>,
        connection: Weak<Connection>,
        publish_done_sender: tokio::sync::mpsc::UnboundedSender<RequestId>,
        receiver_span: Span,
    ) -> JoinHandle {
        executor::spawn(
            "Control Message Receiver",
                async move {
                    loop {
                        if let (Some(session), Some(connection)) =
                            (session_context.upgrade(), connection.upgrade())
                        {
                            let received_message = match receive_stream.receive().await {
                                Ok(Some(received_message)) => received_message,
                                Ok(None) => {
                                    tracing::info!("Stream ended.");
                                    break;
                                }
                                Err(error) => {
                                    tracing::info!(%error, "Stream closed.");
                                    break;
                                }
                            };

                            match Self::resolve_message(&session, &connection, received_message) {
                                DepacketizeResult::SessionEvent(event) => {
                                    if let SessionEvent::PublishDone(handler) = &event {
                                        let _ = publish_done_sender.send(handler.request_id());
                                    }
                                    if let Err(error) = connection.event_sender.send(event) {
                                        tracing::error!("failed to send message: {:?}", error);
                                    }
                                }
                                DepacketizeResult::SessionClosed => break,
                                DepacketizeResult::ResponseMessage(request_id, message) => {
                                    let inflight_request = ControlChannel::of(&connection)
                                        .take_inflight_request(request_id);
                                    match inflight_request {
                                        Some(InflightRequest::Waiting {
                                            sender,
                                            on_late_response,
                                        }) => {
                                            // The caller can stop waiting between response
                                            // arrival and this send; fall back to the
                                            // late-response withdrawal in that case.
                                            if let Err(message) = sender.send(message) {
                                                handle_late_response(
                                                    &session,
                                                    &connection,
                                                    request_id,
                                                    on_late_response,
                                                    message,
                                                )
                                                .await;
                                            }
                                        }
                                        Some(InflightRequest::Abandoned(action)) => {
                                            handle_late_response(
                                                &session, &connection, request_id, action,
                                                message,
                                            )
                                            .await;
                                        }
                                        None => {
                                            tracing::error!(
                                                request_id,
                                                "Protocol violation: response for unknown or already-completed Request ID; closing session"
                                            );
                                            session.close_connection(
                                                &connection,
                                                TerminationErrorCode::ProtocolViolation,
                                                "received response for unknown or already-completed Request ID",
                                            );
                                            break;
                                        }
                                    }
                                }
                            }
                        } else {
                            tracing::debug!("Control Message Receiver stopped: the session was dropped");
                            break;
                        }
                    }
                    if let Some(connection) = connection.upgrade() {
                        ControlChannel::of(&connection).fail_inflight_requests();
                    }
                }
            .instrument(receiver_span),
        )
    }

    fn resolve_message(
        session: &Arc<SessionContext>,
        connection: &Arc<Connection>,
        received_message: ReceivedMessage,
    ) -> DepacketizeResult {
        tracing::debug!(message = ?received_message, "Event: message_type");
        match received_message {
            ReceivedMessage::Subscribe(subscribe) => {
                let subscribe_handler =
                    SubscribeHandler::new(session.clone(), connection.clone(), subscribe);
                DepacketizeResult::SessionEvent(SessionEvent::Subscribe(subscribe_handler))
            }
            ReceivedMessage::Unsubscribe(unsubscribe) => {
                let unsubscribe_handler =
                    UnsubscribeHandler::new(session.clone(), connection, unsubscribe);
                DepacketizeResult::SessionEvent(SessionEvent::Unsubscribe(unsubscribe_handler))
            }
            ReceivedMessage::SubscribeOk(subscribe_ok) => {
                let request_id = subscribe_ok.request_id;
                let response = ResponseMessage::SubscribeOk(subscribe_ok);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::SubscribeError(subscribe_error) => {
                let response = ResponseMessage::SubscribeError(
                    subscribe_error.request_id,
                    error_codes::request_error_code_from_wire(
                        subscribe_error.error_code,
                        RequestKind::Subscribe,
                    ),
                    subscribe_error.reason_phrase,
                );
                DepacketizeResult::ResponseMessage(subscribe_error.request_id, response)
            }
            ReceivedMessage::Publish(publish) => {
                let publish_handler =
                    PublishHandler::new(session.clone(), connection.clone(), publish);
                DepacketizeResult::SessionEvent(SessionEvent::Publish(publish_handler))
            }
            ReceivedMessage::PublishOk(publish_ok) => {
                let request_id = publish_ok.request_id;
                let response = ResponseMessage::PublishOk(publish_ok);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::PublishError(publish_error) => {
                let request_id = publish_error.request_id;
                let error_code = error_codes::request_error_code_from_wire(
                    publish_error.error_code,
                    RequestKind::Publish,
                );
                let response = ResponseMessage::PublishError(
                    request_id,
                    error_code,
                    publish_error.reason_phrase,
                );
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::PublishNamespace(publish_namespace) => {
                let publish_namespace_handler =
                    PublishNamespaceHandler::new(connection.clone(), publish_namespace);
                DepacketizeResult::SessionEvent(SessionEvent::PublishNamespace(
                    publish_namespace_handler,
                ))
            }
            ReceivedMessage::PublishNamespaceDone(publish_namespace_done) => {
                let publish_namespace_done_handler =
                    PublishNamespaceDoneHandler::new(publish_namespace_done);
                DepacketizeResult::SessionEvent(SessionEvent::PublishNamespaceDone(
                    publish_namespace_done_handler,
                ))
            }
            ReceivedMessage::PublishNamespaceOk(publish_namespace_ok) => {
                let request_id = publish_namespace_ok.request_id;
                let response = ResponseMessage::PublishNamespaceOk(request_id);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::PublishNamespaceError(publish_namespace_error) => {
                let request_id = publish_namespace_error.request_id;
                let error_code = error_codes::request_error_code_from_wire(
                    publish_namespace_error.error_code,
                    RequestKind::PublishNamespace,
                );
                let reason_phrase = publish_namespace_error.reason_phrase;
                let response =
                    ResponseMessage::PublishNamespaceError(request_id, error_code, reason_phrase);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::SubscribeNamespace(subscribe_namespace) => {
                let subscribe_namespace_handler =
                    SubscribeNamespaceHandler::new(connection.clone(), subscribe_namespace);
                DepacketizeResult::SessionEvent(SessionEvent::SubscribeNameSpace(
                    subscribe_namespace_handler,
                ))
            }
            ReceivedMessage::SubscribeNamespaceOk(subscribe_namespace_ok) => {
                let request_id = subscribe_namespace_ok.request_id;
                let response = ResponseMessage::SubscribeNameSpaceOk(request_id);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::SubscribeNamespaceError(subscribe_namespace_error) => {
                let request_id = subscribe_namespace_error.request_id;
                let error_code = error_codes::request_error_code_from_wire(
                    subscribe_namespace_error.error_code,
                    RequestKind::SubscribeNamespace,
                );
                let reason_phrase = subscribe_namespace_error.reason_phrase;
                let response =
                    ResponseMessage::SubscribeNameSpaceError(request_id, error_code, reason_phrase);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::UnsubscribeNamespace(unsubscribe_namespace) => {
                let unsubscribe_namespace_handler =
                    UnsubscribeNamespaceHandler::new(unsubscribe_namespace);
                DepacketizeResult::SessionEvent(SessionEvent::UnsubscribeNamespace(
                    unsubscribe_namespace_handler,
                ))
            }
            ReceivedMessage::Fetch(fetch) => {
                let fetch_handler = FetchHandler::new(session.clone(), connection.clone(), fetch);
                DepacketizeResult::SessionEvent(SessionEvent::Fetch(fetch_handler))
            }
            ReceivedMessage::FetchOk(fetch_ok) => {
                let request_id = fetch_ok.request_id;
                let response = ResponseMessage::FetchOk(fetch_ok);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::FetchError(fetch_error) => {
                let response = ResponseMessage::FetchError(
                    fetch_error.request_id,
                    error_codes::request_error_code_from_wire(
                        fetch_error.error_code,
                        RequestKind::Fetch,
                    ),
                    fetch_error.reason_phrase,
                );
                DepacketizeResult::ResponseMessage(fetch_error.request_id, response)
            }
            ReceivedMessage::SubscribeUpdate(subscribe_update) => {
                let subscribe_update_handler =
                    SubscribeUpdateHandler::new(connection, subscribe_update);
                DepacketizeResult::SessionEvent(SessionEvent::SubscribeUpdate(
                    subscribe_update_handler,
                ))
            }
            ReceivedMessage::PublishDone(publish_done) => {
                let publish_done_handler = PublishDoneHandler::new(
                    publish_done.request_id,
                    error_codes::publish_done_code_from_wire(publish_done.status_code)
                        .unwrap_or(PublishDoneCode::InternalError),
                    publish_done.stream_count,
                    publish_done.error_reason,
                );
                DepacketizeResult::SessionEvent(SessionEvent::PublishDone(publish_done_handler))
            }
            ReceivedMessage::FetchCancel(fetch_cancel) => {
                let fetch_cancel_handler =
                    FetchCancelHandler::new(session.clone(), connection, fetch_cancel);
                DepacketizeResult::SessionEvent(SessionEvent::FetchCancel(fetch_cancel_handler))
            }
            ReceivedMessage::PublishNamespaceCancel(publish_namespace_cancel) => {
                // §9.27: PUBLISH_NAMESPACE_CANCEL uses the PUBLISH_NAMESPACE_ERROR codes.
                let publish_namespace_cancel_handler = PublishNamespaceCancelHandler::new(
                    publish_namespace_cancel.track_namespace.join("/"),
                    error_codes::request_error_code_from_wire(
                        publish_namespace_cancel.error_code,
                        RequestKind::PublishNamespace,
                    ),
                    publish_namespace_cancel.error_reason,
                );
                DepacketizeResult::SessionEvent(SessionEvent::PublishNamespaceCancel(
                    publish_namespace_cancel_handler,
                ))
            }
            ReceivedMessage::GoAway(go_away) => {
                if let Err(reason) = connection.note_go_away_received(&go_away.new_session_uri) {
                    tracing::error!("Protocol violation: {reason}");
                    session.close_connection(
                        connection,
                        TerminationErrorCode::ProtocolViolation,
                        reason,
                    );
                    return DepacketizeResult::SessionClosed;
                }
                let go_away_handler = GoAwayHandler::new(go_away);
                DepacketizeResult::SessionEvent(SessionEvent::GoAway(go_away_handler))
            }
            ReceivedMessage::MaxRequestId(max_request_id) => {
                let max_request_id_handler = MaxRequestIdHandler::new(max_request_id);
                DepacketizeResult::SessionEvent(SessionEvent::MaxRequestId(max_request_id_handler))
            }
            ReceivedMessage::RequestsBlocked(requests_blocked) => {
                let requests_blocked_handler = RequestsBlockedHandler::new(requests_blocked);
                DepacketizeResult::SessionEvent(SessionEvent::RequestsBlocked(
                    requests_blocked_handler,
                ))
            }
            ReceivedMessage::TrackStatus(track_status) => {
                let track_status_handler =
                    TrackStatusHandler::new(connection.clone(), track_status);
                DepacketizeResult::SessionEvent(SessionEvent::TrackStatus(track_status_handler))
            }
            ReceivedMessage::TrackStatusOk(track_status_ok) => {
                let request_id = track_status_ok.request_id;
                let response = ResponseMessage::TrackStatusOk(track_status_ok);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::TrackStatusError(track_status_error) => {
                let response = ResponseMessage::TrackStatusError(
                    track_status_error.request_id,
                    error_codes::request_error_code_from_wire(
                        track_status_error.error_code,
                        RequestKind::TrackStatus,
                    ),
                    track_status_error.reason_phrase,
                );
                DepacketizeResult::ResponseMessage(track_status_error.request_id, response)
            }
            ReceivedMessage::ClientSetup(_) | ReceivedMessage::ServerSetup(_) => {
                tracing::error!(
                    "Protocol violation: SETUP received after the session was established"
                );
                session.close_connection(
                    connection,
                    TerminationErrorCode::ProtocolViolation,
                    "SETUP received after the session was established",
                );
                DepacketizeResult::SessionClosed
            }
            ReceivedMessage::FatalError(reason) => {
                tracing::error!(%reason, "Protocol violation: malformed control message payload");
                session.close_connection(
                    connection,
                    TerminationErrorCode::ProtocolViolation,
                    "malformed control message payload",
                );
                DepacketizeResult::SessionClosed
            }
        }
    }
}
