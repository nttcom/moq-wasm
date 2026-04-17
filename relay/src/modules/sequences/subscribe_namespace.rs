use crate::modules::{
    core::handler::subscribe_namespace::SubscribeNamespaceHandler,
    sequences::{notifier::Notifier, tables::table::Table},
    types::SessionId,
};
use tracing::Span;

pub(crate) struct SubscribeNameSpace;

impl SubscribeNameSpace {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe_namespace",
        skip_all,
        parent = session_span,
        fields(session_id = %session_id)
    )]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &dyn Table,
        notifier: &Notifier,
        handler: &dyn SubscribeNamespaceHandler,
    ) {
        let track_namespace_prefix = handler.track_namespace_prefix();
        tracing::info!(
            session_id = %session_id,
            track_namespace_prefix = %track_namespace_prefix,
            "SequenceHandler::SubscribeNamespace"
        );
        let track_namespace_prefix = self.register(session_id, table, handler).await;
        self.broadcast_to_subscribers(session_id, &track_namespace_prefix, notifier, table)
            .await;
        self.response(handler).await;
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe_namespace.register",
        skip_all,
        fields(session_id = %session_id, track_namespace_prefix = %handler.track_namespace_prefix())
    )]
    async fn register(
        &self,
        session_id: SessionId,
        table: &dyn Table,
        handler: &dyn SubscribeNamespaceHandler,
    ) -> String {
        let track_namespace_prefix = handler.track_namespace_prefix();
        table.register_subscribe_namespace(session_id, track_namespace_prefix.to_string());
        track_namespace_prefix.to_string()
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe_namespace.broadcast_to_subscribers",
        skip_all,
        fields(session_id = %session_id, track_namespace_prefix = %track_namespace_prefix)
    )]
    async fn broadcast_to_subscribers(
        &self,
        session_id: SessionId,
        track_namespace_prefix: &str,
        notifier: &Notifier,
        table: &dyn Table,
    ) {
        let filtered = table.get_subscribers(track_namespace_prefix).await;
        for (track_namespace, publish_values) in filtered {
            if let (Some(track_name), Some(_track_alias)) = publish_values {
                if notifier
                    .publish(session_id, track_namespace.clone(), track_name.clone())
                    .await
                    .is_some()
                {
                    tracing::info!(
                        "Forwarded PUBLISH '{}' to session:{}",
                        track_namespace,
                        session_id
                    )
                } else {
                    tracing::warn!("Failed to forward PUBLISH: {}", session_id);
                }
            } else if notifier
                .publish_namespace(session_id, track_namespace.clone())
                .await
            {
                tracing::info!(
                    "Forwarded PUBLISH_NAMESPACE '{}' to {}",
                    track_namespace,
                    session_id
                );
            } else {
                tracing::error!("Failed to forward PUBLISH_NAMESPACE");
            }
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe_namespace.response",
        skip_all
    )]
    async fn response(&self, handler: &dyn SubscribeNamespaceHandler) {
        match handler.ok().await {
            Ok(_) => (),
            Err(e) => {
                tracing::error!("Subscribe Namespace Error: {:?}", e);
            }
        }
    }
}
