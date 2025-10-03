use std::sync::Arc;

use uuid::Uuid;

use crate::modules::{
    enums::{Authorization, RequestId},
    namespace_table::NamespaceTable,
    repositories::session_repository::SessionRepository,
    thread_manager::ThreadManager,
};

type Namespace = String;

pub(crate) struct SequenceHandler {
    namespace_table: NamespaceTable,
    session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    thread_manager: ThreadManager,
}

impl SequenceHandler {
    pub(crate) fn new(session_repo: Arc<tokio::sync::Mutex<SessionRepository>>) -> Self {
        Self {
            namespace_table: NamespaceTable::new(),
            session_repo,
            thread_manager: ThreadManager::new(),
        }
    }

    pub(crate) fn publish_namespace(
        &self,
        uuid: Uuid,
        request_id: RequestId,
        track_namespaces: Vec<Namespace>,
        authotization: Vec<Authorization>,
    ) {
        for namespace in track_namespaces {
            self.namespace_table.add(namespace, uuid);
        }
    }

    pub(crate) fn subscribe_namespace(
        &self,
        uuid: Uuid,
        request_id: RequestId,
        track_namespaces: Vec<Namespace>,
        authotization: Vec<Authorization>,
    ) {
    }

    pub(crate) fn publish(&self) {}

    pub(crate) fn subscribe(&self) {}
}
