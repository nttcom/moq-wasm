use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    core::handler::unsubscribe::UnsubscribeHandler,
    relay::{egress::coordinator::EgressCommand, ingress::ingress_coordinator::IngressCommand},
    sequences::tables::table::{LocalPubSubDirectory, UpstreamSubscriptionOrigin},
    types::SessionId,
};
use tracing::Span;

pub(crate) struct Unsubscribe;

impl Unsubscribe {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.unsubscribe",
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
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        handler: Box<dyn UnsubscribeHandler>,
    ) {
        let subscribe_id = handler.subscribe_id();
        tracing::info!(
            session_id = %session_id,
            subscribe_id = %subscribe_id,
            "SequenceHandler::unsubscribe"
        );

        let Some(removed) = table.remove_downstream_subscription(session_id, subscribe_id) else {
            tracing::warn!(
                session_id = %session_id,
                subscribe_id = %subscribe_id,
                "active downstream subscription not found"
            );
            return;
        };

        if egress_sender
            .send(EgressCommand::StopReader {
                subscriber_session_id: session_id,
                downstream_subscribe_id: subscribe_id,
            })
            .await
            .is_err()
        {
            tracing::error!("Failed to send EgressStopRequest.");
        }

        tracing::info!(
            session_id = %session_id,
            subscribe_id = %subscribe_id,
            upstream_session_id = %removed.upstream_key.publisher_session_id,
            track_namespace = %removed.upstream_key.track_namespace,
            track_name = %removed.upstream_key.track_name,
            remaining_downstream_subscriber_count = removed.remaining_downstream_subscriber_count,
            "downstream unsubscribe processed"
        );

        if removed.remaining_downstream_subscriber_count == 0
            && removed.upstream_origin == UpstreamSubscriptionOrigin::Subscribe
        {
            if let Err(err) = forwarder
                .unsubscribe(
                    removed.upstream_key.publisher_session_id,
                    removed.upstream_request_id,
                )
                .await
            {
                tracing::warn!(
                    ?err,
                    upstream_session_id = %removed.upstream_key.publisher_session_id,
                    request_id = %removed.upstream_request_id,
                    "failed to forward upstream unsubscribe"
                );
            } else {
                tracing::info!(
                    upstream_session_id = %removed.upstream_key.publisher_session_id,
                    request_id = %removed.upstream_request_id,
                    "forwarded upstream unsubscribe"
                );
            }

            if ingress_sender
                .send(IngressCommand::StopTrack {
                    track_key: removed.track_key,
                })
                .await
                .is_err()
            {
                tracing::error!(
                    track_key = removed.track_key,
                    "failed to send ingress stop request"
                );
            }
        }
    }
}
