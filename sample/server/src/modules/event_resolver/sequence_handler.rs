use std::sync::Arc;

use dashmap::DashSet;

use crate::modules::{
    core::{
        handler::{
            self, publish::PublishHandler, publish_namespace::PublishNamespaceHandler,
            subscribe::SubscribeHandler, subscribe_namespace::SubscribeNamespaceHandler,
        },
        subscriber::Acceptance,
    },
    enums::{FilterType, Location},
    relaies::{relay::Relay, relay_manager::RelayManager, relay_properties::RelayProperties},
    relations::Relations,
    repositories::session_repository::SessionRepository,
    types::{GroupOrder, SessionId},
};

pub(crate) struct SequenceHandler {
    tables: Relations,
    session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    relay_manager: RelayManager,
}

impl SequenceHandler {
    pub(crate) fn new(session_repo: Arc<tokio::sync::Mutex<SessionRepository>>) -> Self {
        Self {
            tables: Relations::new(),
            session_repo,
            relay_manager: RelayManager::new(),
        }
    }

    pub(crate) async fn publish_namespace(
        &mut self,
        session_id: SessionId,
        handler: Box<dyn PublishNamespaceHandler>,
    ) {
        tracing::info!("publish namespace");
        let track_namespace = handler.track_namespace();

        if let Some(dash_set) = self
            .tables
            .publisher_namespaces
            .get_mut(handler.track_namespace())
        {
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
                .insert(track_namespace.to_string(), dash_set);
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
            .filter(|entry| entry.key().starts_with(track_namespace))
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
                    .send_publish_namespace(track_namespace.to_string())
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

        match handler.ok().await {
            Ok(_) => tracing::info!("OK"),
            Err(e) => tracing::error!("Publish Namespace Error: {:?}", e),
        }
    }

    pub(crate) async fn subscribe_namespace(
        &mut self,
        session_id: SessionId,
        handler: Box<dyn SubscribeNamespaceHandler>,
    ) {
        tracing::info!("subscribe namespace");
        let track_namespace_prefix = handler.track_namespace_prefix();
        if let Some(dash_set) = self
            .tables
            .subscriber_namespaces
            .get_mut(track_namespace_prefix)
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
                .insert(track_namespace_prefix.to_string(), dash_set);
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
            if entry.key().starts_with(track_namespace_prefix) {
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

    pub(crate) async fn publish(&self, session_id: SessionId, handler: Box<dyn PublishHandler>) {
        tracing::info!("publish");
        let track_namespace = handler.track_namespace();
        let track_name = handler.track_name();
        let track_alias = handler.track_alias();

        let full_track_namespace = format!("{}:{}", track_namespace, track_name);

        tracing::info!(
            "New track '{}' (alias '{}') has been published.",
            full_track_namespace,
            track_alias
        );
        self.tables
            .published_tracks
            .insert(full_track_namespace.clone(), session_id);
        tracing::debug!("publisher_tracks: {:?}", self.tables.published_tracks);
        // The draft defines that the relay requires to send `PUBLISH_NAMESPACE` message to
        // any subscriber that has interests in the namespace
        // https://datatracker.ietf.org/doc/draft-ietf-moq-transport/

        // Convert DashMap<Namespace, DashSet<Uuid>> to DashMap<Uuid, DashSet<Namespace>>
        let combined = DashSet::new();
        self.tables
            .subscriber_namespaces
            .iter()
            .filter(|entry| entry.key().starts_with(track_namespace))
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
                    .send_publish(
                        track_namespace.to_string(),
                        track_name.to_string(),
                        track_alias,
                    )
                    .await
                {
                    Ok(_) => {
                        tracing::info!("Sent publish '{}' to {}", full_track_namespace, session_id)
                    }
                    Err(_) => tracing::error!("Failed to send publish"),
                }
            } else {
                tracing::warn!("No publisher");
            }
        }
    }

    pub(crate) async fn subscribe(
        &self,
        session_id: SessionId,
        handler: Box<dyn SubscribeHandler>,
    ) {
        tracing::info!("subscribe");
        let track_namespace = handler.track_namespace();
        let track_name = handler.track_name();
        let full_track_namespace = format!("{}:{}", track_namespace, track_name);
        tracing::debug!("New track '{}' has been subscribed.", full_track_namespace,);

        if let Some(subscriber_id) = self.tables.published_tracks.get(&full_track_namespace) {
            if let Some(subscriber) = self
                .session_repo
                .lock()
                .await
                .get_subscriber(*subscriber_id.value())
                .await
            {
                let _ = subscriber
                    .send_subscribe(track_namespace.to_string(), track_name.to_string())
                    .await
                    .inspect_err(|_| tracing::error!("Failed to send subscribe"));
            }
        }
        if let Some(publisher) = self
            .session_repo
            .lock()
            .await
            .get_publisher(session_id)
            .await
        {
            let datagram_sender = publisher.create_datagram(track_alias);
            if self.relay_manager.relay_map.contains_key(&track_alias) {
                self.relay_manager
                    .relay_map
                    .get_mut(&track_alias)
                    .unwrap()
                    .add_object_sender(track_alias, datagram_sender);
            } else {
                let mut relay = Relay {
                    relay_properties: RelayProperties::new(),
                };
                relay.add_object_sender(track_alias, datagram_sender);
                self.relay_manager.relay_map.insert(track_alias, relay);
            }
        } else {
            tracing::warn!("No publisher");
        }
    }
}
