use crate::modules::{
    core::handler::subscribe::SubscribeHandler,
    relay::{
        egress::coordinator::{EgressCommand, EgressStartRequest},
        ingress::ingress_coordinator::IngressStartRequest,
    },
    sequences::{notifier::Notifier, tables::table::Table},
    types::{SessionId, compose_session_track_key},
};
use tracing::Span;

pub(crate) struct Subscribe;

impl Subscribe {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe",
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
        ingress_sender: &tokio::sync::mpsc::Sender<IngressStartRequest>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        handler: Box<dyn SubscribeHandler>,
    ) {
        let track_namespace = handler.track_namespace();
        let track_name = handler.track_name();
        tracing::info!(
            session_id = %session_id,
            track_namespace = %track_namespace,
            track_name = %track_name,
            "SequenceHandler::subscribe"
        );

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
                ingress_sender,
                egress_sender,
                handler.as_ref(),
            )
            .await;
        } else {
            self.response_error(handler.as_ref()).await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe.relay_subscribe",
        skip_all,
        fields(session_id = %session_id, pub_session_id = %pub_session_id, track_namespace = %track_namespace, track_name = %track_name)
    )]
    async fn relay_subscribe(
        &self,
        session_id: SessionId,
        pub_session_id: SessionId,
        track_namespace: &str,
        track_name: &str,
        notifier: &Notifier,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressStartRequest>,
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

        if ingress_sender
            .send(IngressStartRequest {
                publisher_session_id: pub_session_id,
                subscription,
            })
            .await
            .is_err()
        {
            tracing::error!("Failed to send IngressStartRequest. Session close.");
            return;
        }

        let Ok(subscriber_track_alias) = handler.ok(expires, content_exists).await else {
            tracing::error!("Failed to send `SUBSCRIBE_OK`. Session close.");
            // TODO: send_unsubscribe
            // TODO: close session
            return;
        };

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

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe.response_error",
        skip_all
    )]
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
