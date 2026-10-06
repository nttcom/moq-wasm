use std::sync::Arc;

use crate::modules::{
    cascading::{
        inter_relay_connection_manager::InterRelayConnectionManager,
        route_registry::RelayRouteRegistry,
    },
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        sequences::{
            CascadingRelayContext,
            downstream_publish::{
                DownstreamPublish, downstream_publish_task::DownstreamPublishTask,
            },
        },
    },
    data_plane::ingress::ingress_coordinator::{IngressCommand, IngressStartRequest},
    domain::{
        error_code::PublishErrorCode,
        pub_sub_directory::{
            InMemoryLocalPubSubDirectory,
            entry::{ActiveUpstreamSubscription, UpstreamSubscriptionOrigin},
        },
        session_id::SessionId,
        session_peer::SessionPeer,
        track_key::TrackKey,
    },
    session::{handler::publish::PublishHandler, subscription::UpstreamSubscription},
};

use moqt::FilterType;
use tracing::Span;

pub(crate) struct Publish;

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
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
        downstream_publish: &DownstreamPublish,
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
        let publisher_peer = super::session_peer(session_id, forwarder).await;
        let is_origin_client = publisher_peer == SessionPeer::Client;

        if let Err(error) = self
            .register_upstream_subscription(
                session_id,
                publisher_peer,
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
                .error(
                    PublishErrorCode::InternalError as u64,
                    "Failed to start ingress for published track".to_string(),
                )
                .await
                .is_err()
            {
                tracing::error!("failed to send PUBLISH_ERROR. close session.");
            }
            return;
        }

        let track_key = TrackKey::new(
            upstream_subscription.track_namespace(),
            upstream_subscription.track_name(),
        );
        self.notify_namespace_subscribers(
            downstream_publish,
            cascading_relay_context,
            &track_key,
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

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish.notify_namespace_subscribers",
        skip_all,
        fields(track_key = %track_key)
    )]
    async fn notify_namespace_subscribers(
        &self,
        downstream_publish: &DownstreamPublish,
        cascading_relay_context: CascadingRelayContext<'_>,
        track_key: &TrackKey,
        is_origin_client: bool,
    ) {
        self.notify_local_namespace_subscribers(downstream_publish, track_key);

        if is_origin_client {
            self.notify_remote_subscribers(
                downstream_publish,
                track_key,
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
        fields(track_key = %track_key)
    )]
    fn notify_local_namespace_subscribers(
        &self,
        downstream_publish: &DownstreamPublish,
        track_key: &TrackKey,
    ) {
        let subscriber_session_ids = downstream_publish
            .table
            .get_namespace_subscribers(&track_key.track_namespace);
        for subscriber_session_id in subscriber_session_ids {
            let _publish_task = DownstreamPublishTask::run(
                downstream_publish.clone(),
                subscriber_session_id,
                track_key.clone(),
            );
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
        publisher_peer: SessionPeer,
        table: &InMemoryLocalPubSubDirectory,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
        handler: Arc<dyn PublishHandler>,
        subscription: &UpstreamSubscription,
    ) -> anyhow::Result<()> {
        let track_key = TrackKey::new(subscription.track_namespace(), subscription.track_name());
        let active_upstream = ActiveUpstreamSubscription {
            upstream_request_id: subscription.request_id(),
            expires: None,
            content_exists: subscription.content_exists(),
            origin: UpstreamSubscriptionOrigin::Publish,
            publisher_peer,
        };

        handler.accept_data_receiver().await;

        if !super::start_ingress(
            ingress_sender,
            IngressStartRequest {
                subscriber_session_id: session_id,
                publisher_session_id: session_id,
                track_key: track_key.clone(),
                subscription: subscription.clone(),
                parent_span: Span::current(),
            },
        )
        .await
        {
            anyhow::bail!("failed to send ingress start request");
        }

        table.register_upstream_subscription(track_key, session_id, active_upstream);
        table.register_publish(session_id, publisher_peer, handler);
        Ok(())
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish.notify_remote_subscribers",
        skip_all,
        fields(track_key = %track_key)
    )]
    async fn notify_remote_subscribers(
        &self,
        downstream_publish: &DownstreamPublish,
        track_key: &TrackKey,
        route_registry: &dyn RelayRouteRegistry,
        inter_relay_connection_manager: &InterRelayConnectionManager,
    ) {
        let routes = match route_registry
            .find_namespace_subscribers(&track_key.track_namespace)
            .await
        {
            Ok(routes) => routes,
            Err(err) => {
                tracing::warn!(?err, "failed to find remote publish subscribers");
                return;
            }
        };

        for relay in routes {
            let Some(session_id) =
                super::connect_relay(inter_relay_connection_manager, &relay).await
            else {
                continue;
            };

            tracing::info!(
                relay_id = %relay.relay_id,
                session_id = session_id,
                "forwarding PUBLISH to remote relay"
            );
            let _publish_task = DownstreamPublishTask::run(
                downstream_publish.clone(),
                session_id,
                track_key.clone(),
            );
        }
    }
}
