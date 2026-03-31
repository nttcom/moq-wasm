use crate::modules::{
    core::handler::subscribe::SubscribeHandler,
    relay::{
        egress::coordinator::{EgressCommand, EgressStartRequest},
        ingest::stream_accepter::{IngestCommand, IngestStartRequest},
    },
    sequences::{notifier::Notifier, tables::table::Table},
    types::SessionId,
};

pub(crate) struct Subscribe;

impl Subscribe {
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        table: &dyn Table,
        notifier: &Notifier,
        ingest_sender: &tokio::sync::mpsc::Sender<IngestCommand>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        handler: Box<dyn SubscribeHandler>,
    ) {
        tracing::info!("SequenceHandler::subscribe: {}", session_id);
        let track_namespace = handler.track_namespace();
        let track_name = handler.track_name();
        let full_track_namespace = format!("{}:{}", track_namespace, track_name);
        tracing::debug!("New track '{}' is subscribed.", full_track_namespace,);

        let active_publish = table
            .find_publish_handler_with(handler.track_namespace(), handler.track_name())
            .await;
        if active_publish.is_some() {
            tracing::info!(
                "active publish already exists for {}/{}. ignore for now",
                track_namespace,
                track_name
            );
        } else if let Some(pub_session_id) = table.get_publish_namespace(track_namespace) {
            self.relay_subscribe(
                session_id,
                pub_session_id,
                track_namespace,
                track_name,
                notifier,
                table,
                ingest_sender,
                egress_sender,
                handler.as_ref(),
            )
            .await;
        } else {
            self.response_error(handler.as_ref()).await;
        }
        tracing::info!("SequenceHandler::subscribe: {} DONE", session_id);
    }

    async fn relay_subscribe(
        &self,
        session_id: SessionId,
        pub_session_id: SessionId,
        track_namespace: &str,
        track_name: &str,
        notifier: &Notifier,
        table: &dyn Table,
        ingest_sender: &tokio::sync::mpsc::Sender<IngestCommand>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        handler: &dyn SubscribeHandler,
    ) {
        if let Ok(subscription) = notifier
            .subscribe(
                pub_session_id,
                track_namespace.to_string(),
                track_name.to_string(),
            )
            .await
        {
            let publisher_track_alias = subscription.track_alias();
            tracing::info!("send `SUBSCRIBE_OK` ok");
            if let Ok(subscriber_track_alias) = handler
                .ok(subscription.expires(), subscription.content_exists())
                .await
            {
                
            } else {
                tracing::error!("Failed to send `SUBSCRIBE_OK`. Session close.");
                // TODO: send_unsubscribe
                // TODO: close session
            }
        } else {
            tracing::warn!("Failed to send `SUBSCRIBE_OK`. Session close.");
        }
    }

    async fn response_error(&self, handler: &dyn SubscribeHandler) {
        let _ = handler
            .error(
                0,
                "Designated namespace and track name do not exist.".to_string(),
            )
            .await
            .inspect(|_| tracing::info!("send `SUBSCRIBE_ERROR` ok"))
            .inspect_err(|_| tracing::error!("Failed to send `SUBSCRIBE_ERROR`. Session close."));
    }
}
