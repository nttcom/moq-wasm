use crate::modules::{
    core::handler::subscribe_namespace::SubscribeNamespaceHandler,
    sequences::{notifier::Notifier, tables::table::Table},
    types::SessionId,
};

pub(crate) struct SubscribeNameSpace;

impl SubscribeNameSpace {
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        table: &dyn Table,
        notifier: &Notifier,
        handler: &dyn SubscribeNamespaceHandler,
    ) {
        tracing::info!("SequenceHandler::subscribe namespace: {}", session_id);
        let track_namespace_prefix = self.register(session_id, table, handler).await;
        self.broadcast_to_subscribers(session_id, &track_namespace_prefix, notifier, table)
            .await;
        self.response(handler).await;
        tracing::info!("SequenceHandler::subscribe namespace: {} DONE", session_id);
    }

    async fn register(
        &self,
        session_id: SessionId,
        table: &dyn Table,
        handler: &dyn SubscribeNamespaceHandler,
    ) -> String {
        let track_namespace_prefix = handler.track_namespace_prefix();
        tracing::info!(
            "New namespace prefix '{}' is subscribed.",
            track_namespace_prefix
        );
        table.register_subscribe_namespace(session_id, track_namespace_prefix.to_string());
        track_namespace_prefix.to_string()
    }

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
                    tracing::info!("Sent publish '{}' to {}", track_namespace, session_id)
                } else {
                    tracing::warn!("Failed to send publish: {}", session_id);
                }
            } else if notifier
                .publish_namespace(session_id, track_namespace.clone())
                .await
            {
                tracing::info!("Sent publish '{}' to {}", track_namespace, session_id);
            } else {
                tracing::error!("Failed to send publish namespace");
            }
        }
    }

    async fn response(&self, handler: &dyn SubscribeNamespaceHandler) {
        match handler.ok().await {
            Ok(_) => tracing::info!("OK"),
            Err(e) => {
                tracing::error!("Subscribe Namespace Error: {:?}", e);
            }
        }
    }
}
