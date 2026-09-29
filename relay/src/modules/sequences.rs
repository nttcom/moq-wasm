pub(crate) mod fetch;
pub(crate) mod malformed_track;
pub(crate) mod publish;
pub(crate) mod publish_namespace;
pub(crate) mod publish_namespace_done;
pub(crate) mod subscribe;
pub(crate) mod subscribe_namespace;
pub(crate) mod tables;
#[cfg(test)]
pub(crate) mod test_fixtures;
pub(crate) mod track_status;
pub(crate) mod unsubscribe;
pub(crate) mod unsubscribe_namespace;
pub(crate) mod upstream_serializer;

use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    inter_relay::InterRelayConnectionManager,
    relay::ingress::ingress_coordinator::IngressCommand,
    route_registry::{RelayInfo, RelayRouteRegistry},
    types::{SessionId, TrackKey},
};

pub(crate) struct CascadingRelayContext<'a> {
    pub(crate) route_registry: &'a dyn RelayRouteRegistry,
    pub(crate) inter_relay_connection_manager: &'a InterRelayConnectionManager,
}

pub(crate) async fn is_origin_client(
    session_id: SessionId,
    forwarder: &ControlMessageForwarder,
) -> bool {
    forwarder
        .repository
        .lock()
        .await
        .is_client_session(session_id)
}

pub(crate) async fn connect_relay(
    inter_relay_connection_manager: &InterRelayConnectionManager,
    relay: &RelayInfo,
) -> Option<SessionId> {
    inter_relay_connection_manager
        .get_or_connect(relay)
        .await
        .inspect_err(
            |err| tracing::warn!(?err, relay_id = %relay.relay_id, "failed to connect relay"),
        )
        .ok()
}

pub(crate) async fn release_upstream(
    forwarder: &ControlMessageForwarder,
    ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
    publisher_session_id: SessionId,
    upstream_request_id: u64,
    track_key: &TrackKey,
) {
    if let Err(err) = forwarder
        .unsubscribe(publisher_session_id, upstream_request_id)
        .await
    {
        tracing::warn!(
            ?err,
            upstream_session_id = %publisher_session_id,
            request_id = %upstream_request_id,
            "failed to forward upstream unsubscribe"
        );
    } else {
        tracing::info!(
            upstream_session_id = %publisher_session_id,
            request_id = %upstream_request_id,
            "forwarded upstream unsubscribe"
        );
    }

    if ingress_sender
        .send(IngressCommand::StopTrack {
            track_key: track_key.clone(),
            publisher_session_id,
        })
        .await
        .is_err()
    {
        tracing::error!(track_key = %track_key, "failed to send ingress stop request");
    }
}
