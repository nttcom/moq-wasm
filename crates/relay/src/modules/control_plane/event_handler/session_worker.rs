use moqt::RequestErrorCode;
use std::sync::Arc;

use moqt::object::ContentExists;
use tokio::sync::mpsc;
use tracing::{Instrument, Span};

use super::{WorkerDeps, session_cleanup::cleanup_session, session_event_span::session_event_span};
use crate::logging::{link_span, relay_hostname};
use crate::modules::{
    auth::{
        anonymous_root::{scope_anonymous_root_prefix, scope_anonymous_root_subscription},
        request_gate::{authorize_request, reject_unauthorized},
        token_refresh::refresh_token,
        verified_token::VerifiedToken,
    },
    control_plane::sequences::{
        CascadingRelayContext, downstream_publish::DownstreamPublish,
        drain_release::ReleaseUnwatchedPublishes, fetch::Fetch, go_away::GoAway,
        malformed_track::MalformedTrackCleanup, publish::Publish,
        publish_namespace::PublishNamespace, publish_namespace_done::PublishNamespaceDone,
        subscribe::Subscribe, subscribe_namespace::SubscribeNameSpace,
        subscribe_update::SubscribeUpdate, track_status::TrackStatus, unsubscribe::Unsubscribe,
        unsubscribe_namespace::UnsubscribeNamespace, upstream_publish_done::UpstreamPublishDone,
    },
    domain::{
        delivery_control::DeliveryControl, pub_sub_directory::entry::PublishDoneReason,
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

    fn downstream_publish(&self) -> DownstreamPublish {
        DownstreamPublish {
            table: self.deps.local_pub_sub_directory.clone(),
            forwarder: self.deps.control_message_forwarder.clone(),
            egress_sender: self.deps.egress_sender.clone(),
            cache_store: self.deps.cache_store.clone(),
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
                    parent: None,
                    "relay.malformed_track_detected",
                    session_id = %session_id,
                    relay_hostname = relay_hostname(),
                    track_namespace = %track_key.track_namespace, track_name = %track_key.track_name,
                );
                link_span(&event_span, &self.session_span);
                MalformedTrackCleanup {}
                    .handle(
                        &track_key,
                        self.deps.local_pub_sub_directory.as_ref(),
                        &self.deps.control_message_forwarder,
                        &self.deps.ingress_sender,
                    )
                    .instrument(event_span)
                    .await;
            }
            EventKind::GoAwaySent => {
                ReleaseUnwatchedPublishes
                    .handle(
                        session_id,
                        self.deps.local_pub_sub_directory.as_ref(),
                        &self.deps.control_message_forwarder,
                        &self.deps.ingress_sender,
                    )
                    .instrument(self.session_span.clone())
                    .await;
            }
            EventKind::DownstreamSubscriptionEnded { request_id } => {
                let event_span = tracing::info_span!(
                    parent: None,
                    "relay.downstream_subscription_ended",
                    session_id = %session_id,
                    relay_hostname = relay_hostname(),
                    request_id,
                );
                link_span(&event_span, &self.session_span);
                Unsubscribe {}
                    .handle(
                        session_id,
                        self.deps.local_pub_sub_directory.as_ref(),
                        &self.deps.control_message_forwarder,
                        &self.deps.ingress_sender,
                        request_id,
                    )
                    .instrument(event_span)
                    .await;
            }
            EventKind::ProtocolViolationDetected { reason } => {
                self.session_span.in_scope(|| {
                    tracing::error!(session_id, %reason, "protocol violation detected; closing session");
                });
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
        let event_span = session_event_span(
            session_id,
            &session_span,
            self.verified_token.as_deref(),
            &event,
        );
        event_span.in_scope(|| tracing::debug!("Received session event"));
        if let Err(denied) = authorize_request(self.verified_token.as_deref(), &event) {
            let reject_span = tracing::info_span!(
                parent: &event_span,
                "relay.session.unauthorized_request",
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
                        &deps.local_pub_sub_directory,
                        &deps.control_message_forwarder,
                        &deps.ingress_sender,
                        &deps.upstream_publisher_resolver,
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
                        deps.local_pub_sub_directory.as_ref(),
                        &deps.control_message_forwarder,
                        &self.downstream_publish(),
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
                        deps.local_pub_sub_directory.as_ref(),
                        &deps.control_message_forwarder,
                        &deps.ingress_sender,
                        &self.downstream_publish(),
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
                        &deps.local_pub_sub_directory,
                        &deps.control_message_forwarder,
                        &deps.ingress_sender,
                        &deps.egress_sender,
                        &deps.upstream_publisher_resolver,
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
                        deps.local_pub_sub_directory.as_ref(),
                        &deps.control_message_forwarder,
                        &deps.ingress_sender,
                        handler.subscribe_id(),
                    )
                    .instrument(event_span)
                    .await;
            }
            MoqtSessionEvent::Fetch(handler) => {
                Fetch {
                    fill_timeout: deps.fetch_fill_timeout,
                }
                .handle(
                    session_id,
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
                        deps.local_pub_sub_directory.as_ref(),
                        handler.subscription_request_id(),
                        DeliveryControl::from_subscribe_update(
                            handler.forward(),
                            handler.start_location(),
                            handler.end_group(),
                        ),
                    )
                });
            }
            MoqtSessionEvent::PublishDone(handler) => {
                UpstreamPublishDone
                    .handle(
                        session_id,
                        handler.request_id(),
                        PublishDoneReason {
                            status_code: handler.status_code(),
                            error_reason: handler.error_reason().to_string(),
                        },
                        deps.local_pub_sub_directory.as_ref(),
                        &deps.ingress_sender,
                        &deps.cache_store,
                        deps.route_registry.as_ref(),
                    )
                    .instrument(event_span)
                    .await;
            }
            MoqtSessionEvent::GoAway(..) => {
                GoAway
                    .handle(
                        session_id,
                        &deps.local_pub_sub_directory,
                        &deps.control_message_forwarder,
                        &deps.ingress_sender,
                        &deps.upstream_publisher_resolver,
                    )
                    .instrument(event_span)
                    .await;
            }
            MoqtSessionEvent::PublishNamespaceCancel(..) | MoqtSessionEvent::FetchCancel(..) => {
                event_span.in_scope(|| {
                    tracing::warn!("Relay handling for this event is not implemented");
                });
            }
            MoqtSessionEvent::Disconnected() | MoqtSessionEvent::ProtocolViolation() => {
                event_span.in_scope(|| {
                    if matches!(event, MoqtSessionEvent::Disconnected()) {
                        tracing::info!(session_id, "session disconnected");
                    } else {
                        tracing::info!(session_id, "session closed for a protocol violation");
                    }
                });
                cleanup_session(session_id, deps)
                    .instrument(event_span)
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
                                RequestErrorCode::InternalError,
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
                handler.error(rejected.code, rejected.reason).await
            }
        };
        if let Err(error) = response {
            tracing::warn!(?error, "failed to answer TRACK_STATUS");
        }
    }
}

#[cfg(test)]
mod tests {
    use moqt::{
        Session, SessionEvent, Subscription,
        object::Location,
        publish::PublishDoneCode,
        subscribe::{FilterType, SubscribeOption},
    };

    use crate::modules::{
        auth::test_support::{app_token, connect_client, spawn_relay_and_connect_client},
        test_support::relay_clients::{
            NAMESPACE, TRACK, TrackPublisher, next_event, publish_namespace, subscribe_namespace,
            subscribe_track,
        },
    };

    /// A subscription starting at group 1, whose End Group can be set to
    /// group 0 so that nothing is left to deliver.
    fn from_group_1() -> SubscribeOption {
        SubscribeOption {
            filter_type: FilterType::AbsoluteStart {
                location: Location {
                    group_id: 1,
                    object_id: 0,
                },
            },
            ..SubscribeOption::default()
        }
    }

    async fn end_before_start(subscriber: &Session, subscription: &Subscription) {
        subscriber
            .subscriber()
            .end_before_group(subscription, 1, true)
            .await
            .unwrap();
    }

    async fn next_publish_done(session: &Session) -> (u64, PublishDoneCode) {
        let event = next_event(session).await;
        let SessionEvent::PublishDone(done) = event else {
            panic!("expected PUBLISH_DONE, got {event:?}");
        };
        (done.request_id(), done.status_code())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn publish_namespace_done_reaches_the_namespace_subscribers() {
        // Arrange
        let (relay, watcher) = spawn_relay_and_connect_client(app_token(Some(""), Some(""))).await;
        let announcer = connect_client(&relay).await;
        subscribe_namespace(&watcher, "APP/room").await;
        publish_namespace(&announcer, "APP/room/x").await;
        let SessionEvent::PublishNamespace(_) = next_event(&watcher).await else {
            panic!("expected PUBLISH_NAMESPACE");
        };

        // Act
        announcer
            .publisher()
            .publish_namespace_done("APP/room/x".to_string())
            .await
            .unwrap();
        let event = next_event(&watcher).await;

        // Assert
        let SessionEvent::PublishNamespaceDone(done) = event else {
            panic!("expected PUBLISH_NAMESPACE_DONE, got {event:?}");
        };
        assert_eq!(done.track_namespace, "APP/room/x");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unsubscribe_of_the_only_subscriber_releases_the_upstream_subscription() {
        // Arrange
        let (relay, subscriber) =
            spawn_relay_and_connect_client(app_token(Some(""), Some(""))).await;
        let mut publisher = TrackPublisher::run(&relay).await;
        let subscription = subscribe_track(&subscriber, SubscribeOption::default()).await;

        // Act
        subscriber
            .subscriber()
            .unsubscribe(&subscription)
            .await
            .unwrap();
        let unsubscribed = publisher.next_unsubscribe().await;

        // Assert
        assert!(
            unsubscribed.is_some(),
            "the relay kept its upstream subscription"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn publish_done_of_the_publisher_ends_the_downstream_subscription() {
        // Arrange
        let (relay, subscriber) =
            spawn_relay_and_connect_client(app_token(Some(""), Some(""))).await;
        let publisher = TrackPublisher::run(&relay).await;
        let subscription = subscribe_track(&subscriber, SubscribeOption::default()).await;

        // Act
        publisher.origin.end_track(NAMESPACE, TRACK).await.unwrap();
        let done = next_publish_done(&subscriber).await;

        // Assert
        assert_eq!(
            done,
            (subscription.request_id(), PublishDoneCode::TrackEnded)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_update_that_leaves_nothing_to_deliver_ends_the_subscription() {
        // Arrange
        let (relay, subscriber) =
            spawn_relay_and_connect_client(app_token(Some(""), Some(""))).await;
        let _publisher = TrackPublisher::run(&relay).await;
        let subscription = subscribe_track(&subscriber, from_group_1()).await;

        // Act
        end_before_start(&subscriber, &subscription).await;
        let done = next_publish_done(&subscriber).await;

        // Assert
        assert_eq!(
            done,
            (
                subscription.request_id(),
                PublishDoneCode::SubscriptionEnded
            )
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_subscription_whose_range_is_delivered_releases_the_upstream_subscription() {
        // Arrange
        let (relay, subscriber) =
            spawn_relay_and_connect_client(app_token(Some(""), Some(""))).await;
        let mut publisher = TrackPublisher::run(&relay).await;
        let subscription = subscribe_track(&subscriber, from_group_1()).await;

        // Act
        end_before_start(&subscriber, &subscription).await;
        let unsubscribed = publisher.next_unsubscribe().await;

        // Assert
        assert!(
            unsubscribed.is_some(),
            "the relay kept its upstream subscription"
        );
    }
}
