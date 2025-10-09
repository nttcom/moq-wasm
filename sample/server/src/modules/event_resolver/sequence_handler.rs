use std::sync::Arc;

use dashmap::{DashMap, DashSet};
use uuid::Uuid;

use crate::modules::{
    repositories::session_repository::SessionRepository, relations::Relations,
    thread_manager::ThreadManager,
};

type Namespace = String;

pub(crate) struct SequenceHandler {
    tables: Arc<Relations>,
    session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    thread_manager: ThreadManager,
}

impl SequenceHandler {
    pub(crate) fn new(session_repo: Arc<tokio::sync::Mutex<SessionRepository>>) -> Self {
        Self {
            tables: Arc::new(Relations::new()),
            session_repo,
            thread_manager: ThreadManager::new(),
        }
    }

    pub(crate) fn publish_namespace(&mut self, uuid: Uuid, track_namespaces: Vec<Namespace>) {
        let table = self.tables.clone();
        let session_repo = self.session_repo.clone();

        let join_handle = tokio::spawn(async move {
            let dest_map = DashMap::new();

            for namespace in track_namespaces {
                if let Some(dash_set) = table.publisher_namespaces.get_mut(&namespace) {
                    tracing::info!("The Namespace has been registered. :{}", namespace);
                    dash_set.insert(uuid);
                } else {
                    tracing::info!("New namespace has been published. :{}", namespace);
                    let dash_set = DashSet::new();
                    dash_set.insert(uuid);
                    table
                        .publisher_namespaces
                        .insert(namespace.clone(), dash_set);
                }
                if let None = table.published_tracks.get_mut(&namespace) {
                    table
                        .published_tracks
                        .insert(namespace.clone(), DashSet::new());
                }
                // The draft defines that the relay requires to send `PUBLISH_NAMESPACE` message to
                // any subscriber that has interests in the namespace
                // https://datatracker.ietf.org/doc/draft-ietf-moq-transport/

                // Convert DashMap<Namespace, DashSet<Uuid>> to DashMap<Uuid, DashSet<Namespace>>
                if let Some(uuids) = table.subscriber_namespaces.get(&namespace) {
                    for uuid in uuids.iter() {
                        let uuid = *uuid;

                        // そのuuidのオブジェクトが持つNamespaceのセットを取得
                        let session = match session_repo.lock().await.get_session(uuid).await {
                            Some(s) => s,
                            None => {
                                tracing::info!("no session subscribes. :{}", namespace);
                                continue;
                            }
                        };
                        let ns = session.get_subscribed_namespaces().await;

                        // 現在のnamespaceがh1に含まれているか確認
                        if ns.contains(&namespace) {
                            // d2にuuidのエントリを取得または作成
                            dest_map
                                .entry(uuid)
                                .or_insert_with(DashSet::new)
                                .insert(namespace.clone());
                        }
                    }
                }
            }

            for (uuid, namespaces) in dest_map {
                let publisher = match session_repo.lock().await.get_publisher(uuid).await {
                    Some(s) => s,
                    None => {
                        tracing::info!("no session publishers. :{}", uuid);
                        continue;
                    }
                };
                if namespaces.len() == 0 {
                    continue;
                }
                publisher.send_publish_namespace(namespaces.into_iter().collect());
            }
        });
        self.thread_manager.add_join_handle(join_handle);
    }

    pub(crate) fn subscribe_namespace(&mut self, uuid: Uuid, track_namespaces: Vec<Namespace>) {
        let table = self.tables.clone();

        let join_handle = tokio::spawn(async move {
            for namespace in track_namespaces.clone() {
                if let Some(dash_set) = table.subscriber_namespaces.get_mut(&namespace) {
                    tracing::info!("The Namespace has been registered. :{}", namespace);
                    dash_set.insert(uuid);
                } else {
                    tracing::info!("New namespace has been subscribed. :{}", namespace);
                    let dash_set = DashSet::new();
                    dash_set.insert(uuid);
                    table
                        .subscriber_namespaces
                        .insert(namespace.clone(), dash_set);
                }
                if let None = table.subscribed_tracks.get_mut(&namespace) {
                    table
                        .subscribed_tracks
                        .insert(namespace.clone(), DashSet::new());
                }
            }
        });
        self.thread_manager.add_join_handle(join_handle);
    }

    pub(crate) fn publish(&self) {}

    pub(crate) fn subscribe(&self) {}
}
