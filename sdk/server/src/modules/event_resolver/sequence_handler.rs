use std::sync::Arc;

use uuid::Uuid;

use crate::modules::{
    repositories::session_repository::SessionRepository, tables::Tables,
    thread_manager::ThreadManager,
};

type Namespace = String;

pub(crate) struct SequenceHandler {
    pub_namespace_table: Arc<tokio::sync::Mutex<Tables>>,
    sub_namespace_table: Arc<tokio::sync::Mutex<Tables>>,
    session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    thread_manager: ThreadManager,
}

impl SequenceHandler {
    pub(crate) fn new(session_repo: Arc<tokio::sync::Mutex<SessionRepository>>) -> Self {
        Self {
            pub_namespace_table: Arc::new(tokio::sync::Mutex::new(Tables::new())),
            sub_namespace_table: Arc::new(tokio::sync::Mutex::new(Tables::new())),
            session_repo,
            thread_manager: ThreadManager::new(),
        }
    }

    pub(crate) fn publish_namespace(&mut self, uuid: Uuid, track_namespaces: Vec<Namespace>) {
        let pub_table = self.pub_namespace_table.clone();
        let sub_table = self.sub_namespace_table.clone();
        let session_repo = self.session_repo.clone();

        let join_handle = tokio::spawn(async move {
            for namespace in track_namespaces.clone() {
                pub_table.lock().await.add(namespace.clone(), uuid);
                let ids = sub_table
                    .lock()
                    .await
                    .get_by_namespace(namespace.clone())
                    .await;
                for id in ids {
                    let publisher = session_repo.lock().await.get_publisher(id).await;
                    if let Some(publisher) = publisher {
                        publisher
                            .send_publish_namespace(vec![namespace.clone()])
                            .await;
                    } else {
                        tracing::error!("publisher not found");
                    }
                }
            }
        });
        self.thread_manager.add_join_handle(join_handle);
    }

    pub(crate) fn subscribe_namespace(&mut self, uuid: Uuid, track_namespaces: Vec<Namespace>) {
        let sub_table = self.sub_namespace_table.clone();

        let join_handle = tokio::spawn(async move {
            for namespace in track_namespaces.clone() {
                sub_table.lock().await.add(namespace, uuid);
            }
        });
        self.thread_manager.add_join_handle(join_handle);
    }

    pub(crate) fn publish(&self) {}

    pub(crate) fn subscribe(&self) {}
}
