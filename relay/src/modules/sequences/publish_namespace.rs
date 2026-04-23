use crate::modules::{
    core::handler::publish_namespace::PublishNamespaceHandler,
    sequences::{notifier::SessionNotifier, tables::table::RelayTable},
    types::SessionId,
};

pub(crate) struct PublishNamespace;

impl PublishNamespace {
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        table: &dyn RelayTable,
        notifier: &SessionNotifier,
        handler: &dyn PublishNamespaceHandler,
    ) {
        tracing::info!("SequenceHandler::publish namespace: {}", session_id);
        let Some(track_namespace) = self.register(session_id, table, handler).await else {
            return;
        };

        // The draft defines that the relay requires to send `PUBLISH_NAMESPACE` message to
        // any subscriber that has interests in the namespace
        // https://datatracker.ietf.org/doc/draft-ietf-moq-transport/

        // Convert DashMap<Namespace, DashSet<SessionId>> to DashMap<SessionId, DashSet<Namespace>>
        self.notify_to_subscribers(&track_namespace, table, notifier)
            .await;

        self.response(handler).await;
        tracing::info!("SequenceHandler::publish namespace: {} DONE", session_id);
    }

    async fn register(
        &self,
        session_id: SessionId,
        table: &dyn RelayTable,
        handler: &dyn PublishNamespaceHandler,
    ) -> Option<String> {
        let track_namespace = handler.track_namespace();

        if !table.register_publish_namespace(session_id, track_namespace.to_string()) {
            // TODO: Session close.
            tracing::error!("Failed to register publish namespace");
            match handler
                .error(0, "Failed to register publish namespace".to_string())
                .await
            {
                Ok(_) => tracing::info!("send `PUBLISH_NAMESPACE_ERROR` ok"),
                Err(_) => {
                    tracing::error!("Failed to send `PUBLISH_NAMESPACE_ERROR`. Session close.")
                }
            }
            None
        } else {
            Some(track_namespace.to_string())
        }
    }

    async fn notify_to_subscribers(
        &self,
        track_namespace: &str,
        table: &dyn RelayTable,
        notifier: &SessionNotifier,
    ) {
        let combined = table.get_namespace_subscribers(track_namespace);
        tracing::debug!("The namespace are subscribed by: {:?}", combined);
        for session_id in combined {
            if notifier
                .publish_namespace(session_id, track_namespace.to_string())
                .await
            {
                tracing::info!(
                    "Sent publish namespace '{}' to {}",
                    track_namespace,
                    session_id
                )
            } else {
                tracing::warn!("Failed to send publish namespace: {}", session_id);
            }
        }
    }

    async fn response(&self, handler: &dyn PublishNamespaceHandler) {
        match handler.ok().await {
            Ok(_) => tracing::info!("OK"),
            Err(e) => {
                tracing::error!("Publish Namespace Error: {:?}", e);
            }
        }
    }
}
