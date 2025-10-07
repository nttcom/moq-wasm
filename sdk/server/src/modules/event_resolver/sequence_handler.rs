use std::sync::Arc;

use dashmap::DashSet;
use uuid::Uuid;

use crate::modules::{
    repositories::session_repository::SessionRepository, tables::Tables,
    thread_manager::ThreadManager,
};

type Namespace = String;

pub(crate) struct SequenceHandler {
    tables: Arc<Tables>,
    session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    thread_manager: ThreadManager,
}

impl SequenceHandler {
    pub(crate) fn new(session_repo: Arc<tokio::sync::Mutex<SessionRepository>>) -> Self {
        Self {
            tables: Arc::new(Tables::new()),
            session_repo,
            thread_manager: ThreadManager::new(),
        }
    }

    pub(crate) fn publish_namespace(&mut self, uuid: Uuid, track_namespaces: Vec<Namespace>) {
        let table = self.tables.clone();
        let session_repo = self.session_repo.clone();

        let join_handle = tokio::spawn(async move {
            for namespace in track_namespaces {
                if let Some(dash_set) = table.publishers.get_mut(&namespace) {
                    tracing::info!("The Namespace has been registered.");
                    dash_set.insert(uuid);
                } else {
                    tracing::info!("New namespace has been published.");
                    let dash_set = DashSet::new();
                    dash_set.insert(uuid);
                    table.publishers.insert(namespace.clone(), dash_set);
                }
                if let None = table.publisher_namespaces.get_mut(&namespace) {
                    table.publisher_namespaces.insert(namespace.clone(), DashSet::new());
                }
            }
        });
        self.thread_manager.add_join_handle(join_handle);
    }

    pub(crate) fn subscribe_namespace(&mut self, uuid: Uuid, track_namespaces: Vec<Namespace>) {
        let table = self.tables.clone();

        let join_handle = tokio::spawn(async move {
            for namespace in track_namespaces.clone() {
                if let Some(dash_set) = table.subscribers.get_mut(&namespace) {
                    tracing::info!("The Namespace has been registered.");
                    dash_set.insert(uuid);
                } else {
                    tracing::info!("New namespace has been subscribed.");
                    let dash_set = DashSet::new();
                    dash_set.insert(uuid);
                    table.subscribers.insert(namespace.clone(), dash_set);
                }
                if let None = table.subscriber_namespaces.get_mut(&namespace) {
                    table.subscriber_namespaces.insert(namespace.clone(), DashSet::new());
                }
            }
        });
        self.thread_manager.add_join_handle(join_handle);
    }

    pub(crate) fn publish(&self) {}

    pub(crate) fn subscribe(&self) {}
}
