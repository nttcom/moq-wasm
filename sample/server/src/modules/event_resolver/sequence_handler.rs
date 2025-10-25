use std::sync::Arc;

use dashmap::DashSet;

use crate::modules::{
    enums::{FilterType, Location},
    relations::Relations,
    repositories::session_repository::SessionRepository,
    types::{GroupOrder, SessionId, TrackNamespace, TrackNamespacePrefix},
};

pub(crate) struct SequenceHandler {
    tables: Relations,
    session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
}

impl SequenceHandler {
    pub(crate) fn new(session_repo: Arc<tokio::sync::Mutex<SessionRepository>>) -> Self {
        Self {
            tables: Relations::new(),
            session_repo,
        }
    }

    pub(crate) async fn publish_namespace(
        &mut self,
        session_id: SessionId,
        track_namespace: TrackNamespace,
    ) {
        tracing::info!("publish namespace");

        if let Some(dash_set) = self.tables.publisher_namespaces.get_mut(&track_namespace) {
            tracing::info!(
                "'{}' has been registered for namespace publication.",
                track_namespace
            );
            dash_set.insert(session_id);
        } else {
            tracing::info!("New namespace '{}' has been subscribed.", track_namespace);
            let dash_set = DashSet::new();
            dash_set.insert(session_id);
            self.tables
                .publisher_namespaces
                .insert(track_namespace.clone(), dash_set);
        }
        tracing::debug!(
            "publisher_namespaces: {:?}",
            self.tables.publisher_namespaces
        );
        // The draft defines that the relay requires to send `PUBLISH_NAMESPACE` message to
        // any subscriber that has interests in the namespace
        // https://datatracker.ietf.org/doc/draft-ietf-moq-transport/

        // Convert DashMap<Namespace, DashSet<Uuid>> to DashMap<Uuid, DashSet<Namespace>>
        let combined = DashSet::new();
        self.tables
            .subscriber_namespaces
            .iter()
            .filter(|entry| entry.key().starts_with(track_namespace.as_str()))
            .for_each(|entry| {
                entry.value().iter().for_each(|session_id| {
                    combined.insert(*session_id);
                })
            });
        tracing::debug!("The namespace are subscribed by: {:?}", combined);
        for session_id in combined {
            let publisher = self
                .session_repo
                .lock()
                .await
                .get_publisher(session_id)
                .await;
            if let Some(publisher) = publisher {
                match publisher
                    .send_publish_namespace(track_namespace.clone())
                    .await
                {
                    Ok(_) => tracing::info!(
                        "Sent publish namespace '{}' to {}",
                        track_namespace,
                        session_id
                    ),
                    Err(_) => tracing::error!("Failed to send publish namespace"),
                }
            } else {
                tracing::warn!("No publisher");
            }
        }
    }

    pub(crate) async fn subscribe_namespace(
        &mut self,
        session_id: SessionId,
        track_namespace_prefix: TrackNamespacePrefix,
    ) {
        tracing::info!("subscribe namespace");

        if let Some(dash_set) = self
            .tables
            .subscriber_namespaces
            .get_mut(&track_namespace_prefix)
        {
            tracing::info!(
                "The namespace prefix '{}' has been registered for namespace subscription.",
                track_namespace_prefix
            );
            dash_set.insert(session_id);
        } else {
            tracing::info!(
                "New namespace prefix '{}' has been subscribed.",
                track_namespace_prefix
            );
            let dash_set = DashSet::new();
            dash_set.insert(session_id);
            self.tables
                .subscriber_namespaces
                .insert(track_namespace_prefix.clone(), dash_set);
        }
        tracing::info!(
            "New namespace prefix '{}' has been subscribed.",
            track_namespace_prefix
        );
        tracing::debug!(
            "subscriber_namespaces: {:?}",
            self.tables.subscriber_namespaces
        );

        tracing::debug!(
            "publisher_namespaces: {:?}",
            self.tables.publisher_namespaces
        );
        let mut filtered = Vec::new();
        for entry in self.tables.publisher_namespaces.iter() {
            if entry.key().starts_with(track_namespace_prefix.as_str()) {
                filtered.push(entry.key().clone());
            }
        }

        tracing::debug!("The namespace prefix are subscribed by: {:?}", filtered);

        for track_namespace in filtered {
            let publisher = self
                .session_repo
                .lock()
                .await
                .get_publisher(session_id)
                .await;
            if let Some(publisher) = publisher {
                match publisher
                    .send_publish_namespace(track_namespace.clone())
                    .await
                {
                    Ok(_) => tracing::info!(
                        "Sent publish namespace '{}' to {}",
                        track_namespace,
                        session_id
                    ),
                    Err(_) => tracing::error!("Failed to send publish namespace"),
                }
            } else {
                tracing::warn!("No publisher");
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn publish(
        &self,
        session_id: SessionId,
        track_namespace: String,
        track_name: String,
        track_alias: u64,
        group_order: GroupOrder,
        is_content_exist: bool,
        location: Option<Location>,
        is_forward: bool,
        delivery_timeout: u64,
        max_cache_duration: u64,
    ) {
        tracing::info!("publish namespace");
        let full_track_namespace = format!("{}:{}", track_namespace, track_name);

        tracing::info!("New namespace '{}' has been subscribed.", track_namespace);
        self.tables
            .published_tracks
            .insert(full_track_namespace.clone(), session_id);
        tracing::debug!(
            "publisher_namespaces: {:?}",
            self.tables.publisher_namespaces
        );
        // The draft defines that the relay requires to send `PUBLISH_NAMESPACE` message to
        // any subscriber that has interests in the namespace
        // https://datatracker.ietf.org/doc/draft-ietf-moq-transport/

        // Convert DashMap<Namespace, DashSet<Uuid>> to DashMap<Uuid, DashSet<Namespace>>
        let combined = DashSet::new();
        self.tables
            .subscriber_namespaces
            .iter()
            .filter(|entry| entry.key().starts_with(track_namespace.as_str()))
            .for_each(|entry| {
                entry.value().iter().for_each(|session_id| {
                    combined.insert(*session_id);
                })
            });
        tracing::debug!("The namespace are subscribed by: {:?}", combined);
        for session_id in combined {
            let publisher = self
                .session_repo
                .lock()
                .await
                .get_publisher(session_id)
                .await;
            if let Some(publisher) = publisher {
                match publisher
                    .send_publish_namespace(track_namespace.clone())
                    .await
                {
                    Ok(_) => tracing::info!(
                        "Sent publish namespace '{}' to {}",
                        track_namespace,
                        session_id
                    ),
                    Err(_) => tracing::error!("Failed to send publish namespace"),
                }
            } else {
                tracing::warn!("No publisher");
            }
        }
        let subscriber = self
            .session_repo
            .lock()
            .await
            .get_subscriber(session_id)
            .await;
        if let Some(subscriber) = subscriber {
            let _ = subscriber
                .send_subscribe(track_namespace, track_name, track_alias)
                .await
                .inspect_err(|_| tracing::error!("Failed to send subscribe"));
            let datagram_receiver = subscriber.accept_datagram();
        }
    }

    pub(crate) fn subscribe(
        &self,
        session_id: SessionId,
        namespaces: String,
        track_name: String,
        track_alias: u64,
        subscriber_priority: u8,
        group_order: GroupOrder,
        is_content_exist: bool,
        is_forward: bool,
        filter_type: FilterType,
        delivery_timeout: u64,
    ) {
    }
}
