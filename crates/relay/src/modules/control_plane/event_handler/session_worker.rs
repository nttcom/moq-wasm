use std::sync::Arc;

use moqt::ContentExists;
use tokio::sync::mpsc;
use tracing::{Instrument, Span};

use super::{WorkerDeps, session_cleanup::cleanup_session, session_event_span::session_event_span};
use crate::modules::{
    auth::{
        anonymous_root::{scope_anonymous_root_prefix, scope_anonymous_root_subscription},
        request_gate::{authorize_request, reject_unauthorized},
        token_refresh::refresh_token,
        verified_token::VerifiedToken,
    },
    control_plane::sequences::{
        CascadingRelayContext, fetch::Fetch, malformed_track::MalformedTrackCleanup,
        publish::Publish, publish_namespace::PublishNamespace,
        publish_namespace_done::PublishNamespaceDone, subscribe::Subscribe,
        subscribe_namespace::SubscribeNameSpace, subscribe_update::SubscribeUpdate,
        track_status::TrackStatus, unsubscribe::Unsubscribe,
        unsubscribe_namespace::UnsubscribeNamespace, upstream_publish_done::UpstreamPublishDone,
    },
    domain::{
        error_code::SubscribeErrorCode, pub_sub_directory::entry::PublishDoneReason,
        session_id::SessionId,
    },
    session::{
        handler::track_status::TrackStatusHandler,
        moqt_session_event::MoqtSessionEvent,
        session_event::{EventKind, SessionEvent},
    },
};

/// Handles one session's events strictly in order: each handler, including
/// its peer round-trips, completes before the next event is pulled.
pub(super) struct SessionWorker {
    session_id: SessionId,
    session_span: Span,
    verified_token: Option<Arc<VerifiedToken>>,
    deps: WorkerDeps,
}

impl SessionWorker {
    pub(super) async fn run(
        session_id: SessionId,
        mut rx: mpsc::UnboundedReceiver<SessionEvent>,
        deps: WorkerDeps,
    ) -> SessionId {
        let (session_span, verified_token) = {
            let repo = deps.repo.lock().await;
            (
                repo.session_span(session_id),
                repo.verified_token(session_id),
            )
        };
        // The span is registered before SessionRegistered is sent and removed
        // only by this worker's terminal cleanup.
        let session_span = session_span
            .unwrap_or_else(|| unreachable!("worker started for a session without a span"));
        if verified_token.is_none() {
            tracing::error!(
                session_id,
                "session has no verified token; every namespace request will be denied"
            );
        }
        let mut worker = Self {
            session_id,
            session_span,
            verified_token,
            deps,
        };

        while let Some(event) = rx.recv().await {
            let is_terminal = matches!(
                event.kind,
                EventKind::FromSession(
                    MoqtSessionEvent::Disconnected() | MoqtSessionEvent::ProtocolViolation()
                )
            );
            worker.handle(event.kind).await;
            if is_terminal {
                break;
            }
        }

        session_id
    }

    fn cascading_relay_context(&self) -> CascadingRelayContext<'_> {
        CascadingRelayContext {
            route_registry: self.deps.route_registry.as_ref(),
            inter_relay_connection_manager: self.deps.inter_relay_connection_manager.as_ref(),
        }
    }

    async fn handle(&mut self, kind: EventKind) {
        let session_id = self.session_id;
        match kind {
            // The reader consumes registrations to spawn this worker and never forwards them.
            EventKind::SessionRegistered => unreachable!("registration forwarded to a worker"),
            EventKind::FromSession(event) => self.handle_moqt_event(event).await,
            EventKind::MalformedTrackDetected(track_key) => {
                let event_span = tracing::info_span!(
                    parent: &self.session_span,
                    "relay.session.event",
                    session_id = %session_id,
                    event = "MalformedTrackDetected",
                    track_key = %track_key,
                );
                MalformedTrackCleanup {}
                    .handle(
                        session_id,
                        &self.session_span,
                        &track_key,
                        self.deps.local_pub_sub_directory.as_ref(),
                        &self.deps.control_message_forwarder,
                        &self.deps.ingress_sender,
                    )
                    .instrument(event_span)
                    .await;
            }
            EventKind::ProtocolViolationDetected { reason } => {
                tracing::error!(
                    parent: &self.session_span,
                    session_id,
                    %reason,
                    "protocol violation detected; closing session"
                );
                self.deps
                    .repo
                    .lock()
                    .await
                    .close_with_protocol_violation(session_id, &reason);
            }
        }
    }

    async fn handle_moqt_event(&mut self, event: MoqtSessionEvent) {
        let session_id = self.session_id;
        let session_span = self.session_span.clone();
        let deps = &self.deps;
        let event = scope_anonymous_root_subscription(self.verified_token.as_deref(), event);
        let event_span = session_event_span(session_id, &session_span, &event);
        event_span.in_scope(|| match event {
            MoqtSessionEvent::ProtocolViolation() => tracing::error!("Received session event"),
            _ => tracing::info!("Received session event"),
        });
        if let Err(denied) = authorize_request(self.verified_token.as_deref(), &event) {
            let reject_span = tracing::info_span!(
                parent: &session_span,
                "relay.session.unauthorized_request",
                session_id = %session_id,
                event = ?event,
                reason = denied.reason,
            );
            reject_unauthorized(event, denied)
                .instrument(reject_span)
                .await;
            return;
        }

        match event {
            MoqtSessionEvent::PublishNamespace(handler) => {
                PublishNamespace {}
                    .handle(
                        session_id,
                        &session_span,
                        deps.local_pub_sub_directory.as_ref(),
                        &deps.control_message_forwarder,
                        self.cascading_relay_context(),
                        handler.as_ref(),
                    )
                    .instrument(event_span)
                    .await;
            }
            MoqtSessionEvent::PublishNamespaceDone(handler) => {
                PublishNamespaceDone {}
                    .handle(
                        session_id,
                        &session_span,
                        deps.local_pub_sub_directory.as_ref(),
                        &deps.control_message_forwarder,
                        self.cascading_relay_context(),
                        &handler,
                    )
                    .instrument(event_span)
                    .await;
            }
            MoqtSessionEvent::SubscribeNamespace(handler) => {
                SubscribeNameSpace {}
                    .handle(
                        session_id,
                        &session_span,
                        deps.local_pub_sub_directory.as_ref(),
                        &deps.control_message_forwarder,
                        deps.route_registry.as_ref(),
                        handler.as_ref(),
                    )
                    .instrument(event_span)
                    .await;
            }
            MoqtSessionEvent::UnsubscribeNamespace(handler) => {
                UnsubscribeNamespace {}
                    .handle(
                        session_id,
                        &session_span,
                        deps.local_pub_sub_directory.as_ref(),
                        &deps.control_message_forwarder,
                        self.cascading_relay_context(),
                        scope_anonymous_root_prefix(
                            self.verified_token.as_deref(),
                            handler.track_namespace_prefix(),
                        ),
                    )
                    .instrument(event_span)
                    .await;
            }
            MoqtSessionEvent::Publish(handler) => {
                Publish {}
                    .handle(
                        session_id,
                        &session_span,
                        deps.local_pub_sub_directory.as_ref(),
                        &deps.control_message_forwarder,
                        &deps.ingress_sender,
                        self.cascading_relay_context(),
                        handler,
                    )
                    .instrument(event_span)
                    .await;
            }
            MoqtSessionEvent::Subscribe(handler) => {
                Subscribe {}
                    .handle(
                        session_id,
                        &session_span,
                        &deps.local_pub_sub_directory,
                        &deps.control_message_forwarder,
                        &deps.ingress_sender,
                        &deps.egress_sender,
                        deps.upstream_publisher_resolver.as_ref(),
                        &deps.cache_store,
                        &deps.upstream_serializer,
                        handler,
                    )
                    .instrument(event_span)
                    .await;
            }
            MoqtSessionEvent::Unsubscribe(handler) => {
                Unsubscribe {}
                    .handle(
                        session_id,
                        &session_span,
                        deps.local_pub_sub_directory.as_ref(),
                        &deps.control_message_forwarder,
                        &deps.ingress_sender,
                        handler,
                    )
                    .instrument(event_span)
                    .await;
            }
            MoqtSessionEvent::Fetch(handler) => {
                Fetch {}
                    .handle(
                        session_id,
                        &session_span,
                        &deps.local_pub_sub_directory,
                        &deps.cache_store,
                        &deps.egress_sender,
                        &deps.relay_event_sender,
                        &deps.control_message_forwarder,
                        &deps.upstream_publisher_resolver,
                        handler,
                    )
                    .instrument(event_span)
                    .await;
            }
            MoqtSessionEvent::TrackStatus(handler) if handler.authorization_tokens().is_empty() => {
                TrackStatus {}
                    .handle(
                        session_id,
                        &session_span,
                        deps.local_pub_sub_directory.as_ref(),
                        &deps.cache_store,
                        handler.as_ref(),
                    )
                    .instrument(event_span)
                    .await;
            }
            MoqtSessionEvent::TrackStatus(handler) => {
                self.refresh_token(handler.as_ref())
                    .instrument(event_span)
                    .await;
            }
            MoqtSessionEvent::SubscribeUpdate(handler) => {
                event_span.in_scope(|| {
                    SubscribeUpdate {}.handle(
                        session_id,
                        &session_span,
                        deps.local_pub_sub_directory.as_ref(),
                        handler.subscription_request_id(),
                        handler.forward(),
                    )
                });
            }
            MoqtSessionEvent::PublishDone(handler) => {
                UpstreamPublishDone
                    .handle(
                        session_id,
                        &session_span,
                        handler.request_id(),
                        PublishDoneReason {
                            status_code: handler.status_code(),
                            error_reason: handler.error_reason().to_string(),
                        },
                        deps.local_pub_sub_directory.as_ref(),
                        &deps.ingress_sender,
                    )
                    .instrument(event_span)
                    .await;
            }
            MoqtSessionEvent::GoAway(..)
            | MoqtSessionEvent::MaxRequestId(..)
            | MoqtSessionEvent::RequestsBlocked(..)
            | MoqtSessionEvent::PublishNamespaceCancel(..)
            | MoqtSessionEvent::FetchCancel(..) => {
                event_span.in_scope(|| {
                    tracing::warn!("Relay handling for this event is not implemented");
                });
            }
            MoqtSessionEvent::Disconnected() | MoqtSessionEvent::ProtocolViolation() => {
                let terminal_span = if matches!(event, MoqtSessionEvent::Disconnected()) {
                    let span = tracing::info_span!(
                        parent: &event_span,
                        "relay.session.disconnected",
                        session_id = session_id
                    );
                    span.in_scope(|| tracing::info!("Session disconnected: {}", session_id));
                    span
                } else {
                    let span = tracing::info_span!(
                        parent: &event_span,
                        "relay.session.protocol_violation",
                        session_id = session_id
                    );
                    span.in_scope(|| tracing::error!("Session protocol violation: {}", session_id));
                    span
                };
                cleanup_session(session_id, deps)
                    .instrument(terminal_span)
                    .await;
            }
        }
    }

    async fn refresh_token(&mut self, handler: &dyn TrackStatusHandler) {
        let refreshed = refresh_token(
            self.deps.token_verifier.as_ref(),
            self.verified_token.as_deref(),
            handler.authorization_tokens(),
        )
        .await;
        let response = match refreshed {
            Ok(token) => {
                let replaced = self
                    .deps
                    .repo
                    .lock()
                    .await
                    .replace_verified_token(self.session_id, token);
                match replaced {
                    Some(token) => {
                        tracing::info!(
                            expires_at = ?token.expires_at,
                            "authorization token refreshed"
                        );
                        self.verified_token = Some(token);
                        handler.ok(0, ContentExists::False).await
                    }
                    None => {
                        handler
                            .error(
                                SubscribeErrorCode::InternalError as u64,
                                "session not found".to_string(),
                            )
                            .await
                    }
                }
            }
            Err(rejected) => {
                tracing::warn!(
                    code = ?rejected.code,
                    reason = %rejected.reason,
                    "authorization token refresh rejected"
                );
                handler.error(rejected.code as u64, rejected.reason).await
            }
        };
        if let Err(error) = response {
            tracing::warn!(?error, "failed to answer TRACK_STATUS");
        }
    }
}
