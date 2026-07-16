pub(crate) mod fetch;
pub(crate) mod publish;
pub(crate) mod publish_namespace;
pub(crate) mod publish_namespace_done;
pub(crate) mod subscribe;
pub(crate) mod subscribe_namespace;
pub(crate) mod tables;
pub(crate) mod unsubscribe;
pub(crate) mod unsubscribe_namespace;
pub(crate) mod upstream_serializer;

use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    inter_relay::InterRelayConnectionManager, route_registry::RelayRouteRegistry,
    types::SessionId,
};

pub(crate) struct CascadingRelayContext<'a> {
    pub(crate) route_registry: &'a dyn RelayRouteRegistry,
    pub(crate) inter_relay_connection_manager: &'a InterRelayConnectionManager,
}

/// True when the message originated from a directly connected client
/// (as opposed to being forwarded by another relay).
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
