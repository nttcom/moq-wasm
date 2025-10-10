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

    pub(crate) async fn publish_namespace(
        &mut self,
        session_id: SessionId,
        track_namespace: TrackNamespace,
    ) {
        let table = self.tables.clone();
        let session_repo = self.session_repo.clone();

        let join_handle = tokio::spawn(async move {
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
            if table.published_tracks.get_mut(&track_namespace).is_none() {
                table
                    .published_tracks
                    .insert(track_namespace.clone(), DashSet::new());
            }
            // The draft defines that the relay requires to send `PUBLISH_NAMESPACE` message to
            // any subscriber that has interests in the namespace
            // https://datatracker.ietf.org/doc/draft-ietf-moq-transport/

            // Convert DashMap<Namespace, DashSet<Uuid>> to DashMap<Uuid, DashSet<Namespace>>
            let combined = DashSet::new();
            table
                .subscriber_namespaces
                .iter()
                .filter(|entry| entry.key().starts_with(track_namespace.as_str()))
                .for_each(|entry| {
                    entry.value().iter().for_each(|session_id| {
                        combined.insert(*session_id);
                    })
                });
            for session_id in combined {
                let publisher = session_repo.lock().await.get_publisher(session_id).await;
                if let Some(publisher) = publisher {
                    match publisher
                        .send_publish_namespace(track_namespace.clone())
                        .await
                    {
                        Ok(_) => tracing::info!("Sent publish namespace"),
                        Err(_) => tracing::error!("Failed to send publish namespace"),
                    }
                } else {
                    tracing::warn!("No publisher");
                }
            }
        });
        self.thread_manager.add_join_handle(join_handle).await;
    }

    pub(crate) async fn subscribe_namespace(
        &mut self,
        session_id: SessionId,
        track_namespace_prefix: TrackNamespacePrefix,
    ) {
        let table = self.tables.clone();
        let session_repo = self.session_repo.clone();

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
            if table
                .subscribed_tracks
                .get_mut(&track_namespace_prefix)
                .is_none()
            {
                table
                    .subscribed_tracks
                    .insert(track_namespace_prefix.clone(), DashSet::new());
            }
            let filtered = DashMap::new();
            for entry in table.publisher_namespaces.iter() {
                if entry.key().starts_with(track_namespace_prefix.as_str()) {
                    filtered.insert(entry.key().clone(), entry.value().clone());
                }
            }

            for (track_namespace, session_ids) in filtered {
                for session_id in session_ids {
                    let publisher = session_repo.lock().await.get_publisher(session_id).await;
                    if let Some(publisher) = publisher {
                        match publisher
                            .send_publish_namespace(track_namespace.clone())
                            .await
                        {
                            Ok(_) => tracing::info!("Sent publish namespace"),
                            Err(_) => tracing::error!("Failed to send publish namespace"),
                        }
                    } else {
                        tracing::warn!("No publisher");
                    }
                }
            }
        });
        self.thread_manager.add_join_handle(join_handle).await;
    }

    pub(crate) fn publish(&self) {}

    pub(crate) fn subscribe(&self) {}
}
