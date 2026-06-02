pub(crate) mod fetch;
pub(crate) mod publish;
pub(crate) mod publish_namespace;
pub(crate) mod subscribe;
pub(crate) mod subscribe_namespace;
pub(crate) mod tables;
pub(crate) mod unsubscribe;
pub(crate) mod unsubscribe_namespace;

use crate::modules::{
    inter_relay::InterRelayConnectionManager, route_registry::RelayRouteRegistry,
};

pub(crate) struct CascadingRelayContext<'a> {
    pub(crate) route_registry: &'a dyn RelayRouteRegistry,
    pub(crate) inter_relay_connection_manager: &'a InterRelayConnectionManager,
}
