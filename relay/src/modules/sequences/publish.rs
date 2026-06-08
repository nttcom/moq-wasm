use std::sync::Arc;

use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    core::{handler::publish::PublishHandler, subscription::UpstreamSubscription},
    enums::FilterType,
    inter_relay::InterRelayConnectionManager,
    relay::ingress::ingress_coordinator::{IngressCommand, IngressStartRequest},
    route_registry::RelayRouteRegistry,
    sequences::{
        CascadingRelayContext,
        tables::table::{
            ActiveUpstreamSubscription, LocalPubSubDirectory, UpstreamSubscriptionKey,
            UpstreamSubscriptionOrigin,
        },
    },
    types::{SessionId, compose_session_track_key},
};
use tracing::Span;

pub(crate) struct Publish;

#[derive(Debug)]
enum RegisterUpstreamSubscriptionError {
    IngressStartFailed,
}

impl RegisterUpstreamSubscriptionError {
    fn code(&self) -> u64 {
        match self {
            Self::IngressStartFailed => 0,
        }
    }

    fn reason_phrase(&self) -> String {
        match self {
            Self::IngressStartFailed => "Failed to start ingress for published track".to_string(),
        }
    }
}

impl Publish {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish",
        skip_all,
        parent = session_span,
        fields(session_id = %session_id)
    )]
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &dyn LocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
        cascading_relay_context: CascadingRelayContext<'_>,
        handler: Box<dyn PublishHandler>,
    ) {
        let handler: Arc<dyn PublishHandler> = Arc::from(handler);
        let upstream_subscription = handler.subscription(0, FilterType::LargestObject);
        tracing::info!(
            session_id = %session_id,
            track_namespace = %upstream_subscription.track_namespace(),
            track_name = %upstream_subscription.track_name(),
            track_alias = %upstream_subscription.track_alias(),
            "SequenceHandler::publish"
        );
        let is_origin_client = self.is_origin_client(session_id, forwarder).await;

        if let Err(error) = self
            .register_upstream_subscription(
                session_id,
                table,
                ingress_sender,
                handler.clone(),
                &upstream_subscription,
            )
            .await
        {
            tracing::error!(
                ?error,
                session_id = %session_id,
                track_namespace = %upstream_subscription.track_namespace(),
                track_name = %upstream_subscription.track_name(),
                "failed to register upstream subscription"
            );
            if handler
                .error(error.code(), error.reason_phrase())
                .await
                .is_err()
            {
                tracing::error!("failed to send PUBLISH_ERROR. close session.");
            }
            return;
        }

        self.notify_namespace_subscribers(
            session_id,
            forwarder,
            table,
            cascading_relay_context,
            &upstream_subscription,
            is_origin_client,
        )
        .await;

        if handler.ok(&upstream_subscription).await.is_err() {
            tracing::error!("failed to send PUBLISH_OK. close session.");
            return;
        }
        tracing::info!(
            session_id = %session_id,
            track_namespace = %upstream_subscription.track_namespace(),
            track_name = %upstream_subscription.track_name(),
            track_alias = %upstream_subscription.track_alias(),
            "SequenceHandler::publish DONE"
        );
    }

    async fn is_origin_client(
        &self,
        session_id: SessionId,
        forwarder: &ControlMessageForwarder,
    ) -> bool {
        forwarder
            .repository
            .lock()
            .await
            .is_client_session(session_id)
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish.notify_namespace_subscribers",
        skip_all,
        fields(publisher_session_id = %publisher_session_id, track_namespace = %subscription.track_namespace(), track_name = %subscription.track_name())
    )]
    async fn notify_namespace_subscribers(
        &self,
        publisher_session_id: SessionId,
        forwarder: &ControlMessageForwarder,
        table: &dyn LocalPubSubDirectory,
        cascading_relay_context: CascadingRelayContext<'_>,
        subscription: &UpstreamSubscription,
        is_origin_client: bool,
    ) {
        self.notify_local_namespace_subscribers(
            publisher_session_id,
            forwarder,
            table,
            subscription,
        )
        .await;

        if is_origin_client {
            self.notify_remote_subscribers(
                subscription.track_namespace(),
                subscription.track_name(),
                forwarder,
                cascading_relay_context.route_registry,
                cascading_relay_context.inter_relay_connection_manager,
            )
            .await;
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish.notify_local_namespace_subscribers",
        skip_all,
        fields(publisher_session_id = %publisher_session_id, track_namespace = %subscription.track_namespace(), track_name = %subscription.track_name())
    )]
    async fn notify_local_namespace_subscribers(
        &self,
        publisher_session_id: SessionId,
        forwarder: &ControlMessageForwarder,
        table: &dyn LocalPubSubDirectory,
        subscription: &UpstreamSubscription,
    ) {
        let track_namespace = subscription.track_namespace().to_string();
        let track_name = subscription.track_name().to_string();
        let track_alias = subscription.track_alias();

        let combined = table.get_namespace_subscribers(&track_namespace);
        for subscriber_session_id in combined {
            if let Some(subscriber_track_alias) = forwarder
                .publish(
                    subscriber_session_id,
                    track_namespace.clone(),
                    track_name.clone(),
                )
                .await
            {
                table.register_track_alias_link(
                    publisher_session_id,
                    track_alias,
                    subscriber_session_id,
                    subscriber_track_alias,
                );
                tracing::info!(
                    "Sent publish '{}' to {}",
                    track_namespace,
                    subscriber_session_id
                );
            } else {
                tracing::warn!("Failed to send publish: {}", subscriber_session_id);
            }
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish.register_upstream_subscription",
        skip_all,
        fields(session_id = %session_id)
    )]
    async fn register_upstream_subscription(
        &self,
        session_id: SessionId,
        table: &dyn LocalPubSubDirectory,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
        handler: Arc<dyn PublishHandler>,
        subscription: &UpstreamSubscription,
    ) -> Result<(), RegisterUpstreamSubscriptionError> {
        let track_namespace = subscription.track_namespace().to_string();
        let track_name = subscription.track_name().to_string();
        let track_key = compose_session_track_key(session_id, subscription.track_alias());
        let upstream_key = UpstreamSubscriptionKey {
            publisher_session_id: session_id,
            track_namespace: track_namespace.clone(),
            track_name: track_name.clone(),
        };
        let active_upstream = ActiveUpstreamSubscription {
            upstream_request_id: subscription.request_id(),
            track_key,
            expires: None,
            content_exists: subscription.content_exists(),
            downstream_subscriber_count: 0,
            origin: UpstreamSubscriptionOrigin::Publish,
        };

        handler.accept_data_receiver().await;

        if ingress_sender
            .send(IngressCommand::Start(Box::new(IngressStartRequest {
                subscriber_session_id: session_id,
                publisher_session_id: session_id,
                track_namespace: track_namespace.clone(),
                track_name: track_name.clone(),
                subscription: subscription.clone(),
                parent_span: Span::current(),
            })))
            .await
            .is_err()
        {
            tracing::error!(
                session_id = %session_id,
                track_namespace = %track_namespace,
                track_name = %track_name,
                "failed to send ingress start request for published track"
            );
            return Err(RegisterUpstreamSubscriptionError::IngressStartFailed);
        }

        table.register_upstream_subscription(upstream_key, active_upstream);
        table.register_publish(session_id, handler).await;
        Ok(())
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish.notify_remote_subscribers",
        skip_all,
        fields(track_namespace = %track_namespace, track_name = %track_name)
    )]
    async fn notify_remote_subscribers(
        &self,
        track_namespace: &str,
        track_name: &str,
        forwarder: &ControlMessageForwarder,
        route_registry: &dyn RelayRouteRegistry,
        inter_relay_connection_manager: &InterRelayConnectionManager,
    ) {
        let routes = match route_registry
            .find_namespace_subscribers(track_namespace)
            .await
        {
            Ok(routes) => routes,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    track_namespace = %track_namespace,
                    track_name = %track_name,
                    "failed to find remote publish subscribers"
                );
                return;
            }
        };

        for relay in routes {
            let session_id = match inter_relay_connection_manager.get_or_connect(&relay).await {
                Ok(session_id) => session_id,
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        relay_id = %relay.relay_id,
                        track_namespace = %track_namespace,
                        track_name = %track_name,
                        "failed to connect remote publish subscriber"
                    );
                    continue;
                }
            };

            if forwarder
                .publish(
                    session_id,
                    track_namespace.to_string(),
                    track_name.to_string(),
                )
                .await
                .is_some()
            {
                tracing::info!(
                    relay_id = %relay.relay_id,
                    session_id = session_id,
                    track_namespace = %track_namespace,
                    track_name = %track_name,
                    "forwarded PUBLISH to remote relay"
                );
            } else {
                tracing::warn!(
                    relay_id = %relay.relay_id,
                    session_id = session_id,
                    track_namespace = %track_namespace,
                    track_name = %track_name,
                    "failed to forward PUBLISH to remote relay"
                );
            }
        }
    }
}
