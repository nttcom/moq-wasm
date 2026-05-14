use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;

use crate::modules::{
    enums::MoqtRelayEvent,
    event_handler::EventHandler,
    relay::{
        egress::coordinator::EgressCoordinator, ingress::ingress_coordinator::IngressCoordinator,
    },
    session_repository::SessionRepository,
};
use crate::relay_server::store::RelayStore;

pub(crate) struct RelayRuntime {
    _ingress: IngressCoordinator,
    _egress: EgressCoordinator,
    _manager: EventHandler,
}

impl RelayRuntime {
    pub(crate) fn new(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        store: &Arc<RelayStore>,
    ) -> (UnboundedSender<MoqtRelayEvent>, Self) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<MoqtRelayEvent>();
        let ingress = IngressCoordinator::new(
            repo.clone(),
            store.cache_store.clone(),
            store.sender_map.clone(),
        );
        let egress = EgressCoordinator::new(
            repo.clone(),
            store.cache_store.clone(),
            store.sender_map.clone(),
        );
        let manager = EventHandler::run(repo, receiver, ingress.sender(), egress.sender());
        (
            sender,
            Self {
                _ingress: ingress,
                _egress: egress,
                _manager: manager,
            },
        )
    }
}
