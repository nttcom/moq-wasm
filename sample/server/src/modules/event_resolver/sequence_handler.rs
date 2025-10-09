use std::sync::Arc;

use dashmap::{DashMap, DashSet};

use crate::modules::{
    relations::Relations,
    repositories::session_repository::SessionRepository,
    thread_manager::ThreadManager,
    types::{SessionId, TrackNamespace, TrackNamespacePrefix},
};

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

    pub(crate) fn publish_namespace(
        &mut self,
        session_id: SessionId,
        track_namespace: TrackNamespace,
    ) {
        let table = self.tables.clone();
        let session_repo = self.session_repo.clone();

        let join_handle = tokio::spawn(async move {
            let dest_map = DashMap::new();

            if let Some(dash_set) = table.publisher_namespaces.get_mut(&track_namespace) {
                tracing::info!("The Namespace has been registered. :{}", track_namespace);
                dash_set.insert(session_id);
            } else {
                tracing::info!("New namespace has been published. :{}", track_namespace);
                let dash_set = DashSet::new();
                dash_set.insert(session_id);
                table
                    .publisher_namespaces
                    .insert(track_namespace.clone(), dash_set);
            }
            if let None = table.published_tracks.get_mut(&track_namespace) {
                table
                    .published_tracks
                    .insert(track_namespace.clone(), DashSet::new());
            }
            // The draft defines that the relay requires to send `PUBLISH_NAMESPACE` message to
            // any subscriber that has interests in the namespace
            // https://datatracker.ietf.org/doc/draft-ietf-moq-transport/

            // Convert DashMap<Namespace, DashSet<Uuid>> to DashMap<Uuid, DashSet<Namespace>>
            if let Some(session_ids) = table.subscriber_namespaces.get(&track_namespace) {
                for session_id in session_ids.iter() {
                    let session_id = *session_id;

                    // そのuuidのオブジェクトが持つNamespaceのセットを取得
                    let session = match session_repo.lock().await.get_session(session_id).await {
                        Some(s) => s,
                        None => {
                            tracing::info!("no session subscribes. :{}", track_namespace);
                            continue;
                        }
                    };
                    let ns = session.get_subscribed_namespaces().await;

                    // 現在のnamespaceがh1に含まれているか確認
                    if ns.contains(&track_namespace) {
                        // d2にuuidのエントリを取得または作成
                        dest_map
                            .entry(session_id)
                            .or_insert_with(DashSet::new)
                            .insert(track_namespace.clone());
                    }
                }
            }

            for (session_id, namespaces) in dest_map {
                let publisher = match session_repo.lock().await.get_publisher(session_id).await {
                    Some(s) => s,
                    None => {
                        tracing::info!("no session publishers. :{}", session_id);
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

    pub(crate) fn subscribe_namespace(
        &mut self,
        session_id: SessionId,
        track_namespace_prefix: TrackNamespacePrefix,
    ) {
        let table = self.tables.clone();

        let join_handle = tokio::spawn(async move {
            if let Some(dash_set) = table.subscriber_namespaces.get_mut(&track_namespace_prefix) {
                tracing::info!(
                    "The Namespace has been registered. :{}",
                    track_namespace_prefix
                );
                dash_set.insert(session_id);
            } else {
                tracing::info!(
                    "New namespace has been subscribed. :{}",
                    track_namespace_prefix
                );
                let dash_set = DashSet::new();
                dash_set.insert(session_id);
                table
                    .subscriber_namespaces
                    .insert(track_namespace_prefix.clone(), dash_set);
            }
            if let None = table.subscribed_tracks.get_mut(&track_namespace_prefix) {
                table
                    .subscribed_tracks
                    .insert(track_namespace_prefix.clone(), DashSet::new());
            }
        });
        self.thread_manager.add_join_handle(join_handle);
    }

    pub(crate) fn publish(&self) {}

    pub(crate) fn subscribe(&self) {}
}
