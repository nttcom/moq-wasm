use std::sync::Arc;

use crate::modules::{
    inter_relay::InterRelayConnectionManager,
    route_registry::RelayRouteRegistry,
    sequences::tables::table::{LocalPubSubDirectory, UpstreamSubscriptionKey},
};

pub(crate) struct UpstreamPublisherResolver {
    route_registry: Arc<dyn RelayRouteRegistry>,
    inter_relay_connection_manager: Arc<InterRelayConnectionManager>,
}

impl UpstreamPublisherResolver {
    pub(crate) fn new(
        route_registry: Arc<dyn RelayRouteRegistry>,
        inter_relay_connection_manager: Arc<InterRelayConnectionManager>,
    ) -> Self {
        Self {
            route_registry,
            inter_relay_connection_manager,
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.upstream_publisher_resolver.resolve",
        skip_all,
        fields(track_namespace = %track_namespace, track_name = %track_name)
    )]
    pub(crate) async fn resolve(
        &self,
        table: &dyn LocalPubSubDirectory,
        track_namespace: &str,
        track_name: &str,
    ) -> anyhow::Result<Option<UpstreamSubscriptionKey>> {
        if let Some(local_publisher) = self
            .find_local_publisher(table, track_namespace, track_name)
            .await
        {
            return Ok(Some(local_publisher));
        }

        self.find_remote_publisher(track_namespace, track_name)
            .await
    }

    async fn find_local_publisher(
        &self,
        table: &dyn LocalPubSubDirectory,
        track_namespace: &str,
        track_name: &str,
    ) -> Option<UpstreamSubscriptionKey> {
        table
            .find_upstream_publishers(track_namespace, track_name)
            .await
            .into_iter()
            .min_by_key(|publisher| publisher.publisher_session_id)
    }

    async fn find_remote_publisher(
        &self,
        track_namespace: &str,
        track_name: &str,
    ) -> anyhow::Result<Option<UpstreamSubscriptionKey>> {
        let Some(relay) = self
            .route_registry
            .find_active_namespace_publisher(track_namespace)
            .await?
        else {
            return Ok(None);
        };

        match self
            .inter_relay_connection_manager
            .get_or_connect(&relay)
            .await
        {
            Ok(publisher_session_id) => {
                tracing::info!(
                    relay_id = %relay.relay_id,
                    publisher_session_id = publisher_session_id,
                    track_namespace = %track_namespace,
                    track_name = %track_name,
                    "resolved remote upstream publisher"
                );
                Ok(Some(UpstreamSubscriptionKey {
                    publisher_session_id,
                    track_namespace: track_namespace.to_string(),
                    track_name: track_name.to_string(),
                }))
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    relay_id = %relay.relay_id,
                    track_namespace = %track_namespace,
                    track_name = %track_name,
                    "failed to connect remote upstream publisher"
                );
                Ok(None)
            }
        }
    }
}
