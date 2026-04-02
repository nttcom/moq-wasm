use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;

use crate::modules::{
    enums::MOQTMessageReceived,
    event_handler::EventHandler,
    relay::{
        egress::coordinator::EgressCoordinator, ingest::ingest_coordinator::IngestCoordinator,
    },
    session_repository::SessionRepository,
};
use crate::relay_server::store::RelayStore;

pub(crate) struct RelayRuntime {
    ingest: IngestCoordinator,
    egress: EgressCoordinator,
    manager: EventHandler,
}

impl RelayRuntime {
    pub(crate) fn new(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        store: &Arc<RelayStore>,
    ) -> (UnboundedSender<MOQTMessageReceived>, Self) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<MOQTMessageReceived>();
        let ingest = IngestCoordinator::new(
            repo.clone(),
            store.cache_store.clone(),
            store.sender_map.clone(),
            store.delivery_type_map.clone(),
        );
        let egress = EgressCoordinator::new(
            repo.clone(),
            store.cache_store.clone(),
            store.sender_map.clone(),
            store.delivery_type_map.clone(),
        );
        let manager = EventHandler::run(repo, receiver, ingest.sender(), egress.sender());
        (
            sender,
            Self {
                ingest,
                egress,
                manager,
            },
        )
    }

    pub(crate) fn is_ingest_running(&self) -> bool {
        self.ingest.is_running()
    }

    pub(crate) fn ingest_track_count(&self) -> usize {
        self.ingest.track_count()
    }

    pub(crate) fn is_egress_running(&self) -> bool {
        self.egress.is_running()
    }

    pub(crate) fn is_manager_running(&self) -> bool {
        self.manager.is_running()
    }
}
