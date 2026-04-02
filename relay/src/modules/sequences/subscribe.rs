use crate::modules::{
    core::handler::subscribe::SubscribeHandler,
    relay::{
        egress::coordinator::{EgressCommand, EgressStartRequest},
        ingest::ingest_coordinator::IngestStartRequest,
    },
    sequences::{notifier::Notifier, tables::table::Table},
    types::{SessionId, compose_session_track_key},
};

pub(crate) struct Subscribe;

impl Subscribe {
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        table: &dyn Table,
        notifier: &Notifier,
        ingest_sender: &tokio::sync::mpsc::Sender<IngestStartRequest>,
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

    #[allow(clippy::too_many_arguments)]
    async fn relay_subscribe(
        &self,
        session_id: SessionId,
        pub_session_id: SessionId,
        track_namespace: &str,
        track_name: &str,
        notifier: &Notifier,
        ingest_sender: &tokio::sync::mpsc::Sender<IngestStartRequest>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        handler: &dyn SubscribeHandler,
    ) {
        let Ok(subscription) = notifier
            .subscribe(
                pub_session_id,
                track_namespace.to_string(),
                track_name.to_string(),
            )
            .await
        else {
            tracing::warn!("Failed to send `SUBSCRIBE` to publisher. Session close.");
            return;
        };

        let track_key = compose_session_track_key(pub_session_id, subscription.track_alias());
        let expires = subscription.expires();
        let content_exists = subscription.content_exists();

        // ingest: upstream から受信してキャッシュに書き込む
        if ingest_sender
            .send(IngestStartRequest {
                publisher_session_id: pub_session_id,
                subscription,
            })
            .await
            .is_err()
        {
            tracing::error!("Failed to send IngestStartRequest. Session close.");
            return;
        }

        // subscriber に SUBSCRIBE_OK を返す
        let Ok(subscriber_track_alias) = handler.ok(expires, content_exists).await else {
            tracing::error!("Failed to send `SUBSCRIBE_OK`. Session close.");
            // TODO: send_unsubscribe
            // TODO: close session
            return;
        };

        // egress: キャッシュから subscriber へ転送
        if egress_sender
            .send(EgressCommand::StartReader(EgressStartRequest {
                subscriber_session_id: session_id,
                track_key,
                published_resources: handler.convert_into_publication(subscriber_track_alias),
            }))
            .await
            .is_err()
        {
            tracing::error!("Failed to send EgressStartRequest. Session close.");
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
