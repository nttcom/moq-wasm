use std::sync::Arc;

use dashmap::DashSet;

use crate::modules::{
    core::handler::{
        publish::{PublishHandler, SubscribeOption},
        publish_namespace::PublishNamespaceHandler,
        subscribe::SubscribeHandler,
        subscribe_namespace::SubscribeNamespaceHandler,
    },
    enums::FilterType,
    event_resolver::stream_binder::StreamBinder,
    relaies::relay_manager::RelayManager,
    relations::Relations,
    repositories::session_repository::SessionRepository,
    types::SessionId,
};

pub(crate) struct SequenceHandler {
    tables: Relations,
    session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    stream_handler: StreamBinder,
}

impl SequenceHandler {
    pub(crate) fn new(session_repo: Arc<tokio::sync::Mutex<SessionRepository>>) -> Self {
        Self {
            tables: Relations::new(),
            session_repo,
            stream_handler: StreamBinder::new(),
        }
    }

    pub(crate) async fn publish_namespace(
        &mut self,
        session_id: SessionId,
        handler: Box<dyn PublishNamespaceHandler>,
    ) {
        tracing::info!("publish namespace");
        let track_namespace = handler.track_namespace();

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
            Err(e) => {
                tracing::error!("Publish Namespace Error: {:?}", e);
                return;
            }
        }

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
    }

    pub(crate) async fn subscribe_namespace(
        &mut self,
        session_id: SessionId,
        handler: Box<dyn SubscribeNamespaceHandler>,
    ) {
        tracing::info!("subscribe namespace");
        let track_namespace_prefix = handler.track_namespace_prefix();
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
        let filtered = DashSet::new();
        for entry in self.tables.publisher_namespaces.iter() {
            if entry.key().starts_with(track_namespace_prefix) {
                filtered.insert((entry.key().clone(), (None, None)));
            }
        }

        for handler in self.tables.published_handlers.read().await.iter() {
            if handler
                .track_namespace()
                .starts_with(track_namespace_prefix)
            {
                filtered.insert((
                    handler.track_namespace().to_string(),
                    (
                        Some(handler.track_name().to_string()),
                        Some(handler.track_alias()),
                    ),
                ));
            }
        }

        tracing::debug!("The namespace prefix are subscribed by: {:?}", filtered);

        for (track_namespace, publish_values) in filtered {
            let publisher = self
                .session_repo
                .lock()
                .await
                .get_publisher(session_id)
                .await;
            if let Some(publisher) = publisher {
                if let (Some(track_name), Some(track_alias)) = publish_values {
                    match publisher
                        .send_publish(track_namespace.clone(), track_name, track_alias)
                        .await
                    {
                        Ok(_) => {
                            tracing::info!("Sent publish '{}' to {}", track_namespace, session_id)
                        }
                        Err(_) => tracing::error!("Failed to send publish"),
                    };
                } else {
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
                }
            } else {
                tracing::warn!("No publisher");
            }
        }

        match handler.ok().await {
            Ok(_) => tracing::info!("OK"),
            Err(e) => {
                tracing::error!("Subscribe Namespace Error: {:?}", e);
                return;
            }
        }

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
    }

    pub(crate) async fn publish(&self, _session_id: SessionId, handler: Box<dyn PublishHandler>) {
        tracing::info!("publish");
        let track_namespace = handler.track_namespace().to_string();
        let track_name = handler.track_name().to_string();
        let track_alias = handler.track_alias();

        let full_track_namespace = format!("{}:{}", track_namespace, track_name);

        tracing::info!(
            "New track '{}' (alias '{}') has been published.",
            full_track_namespace,
            track_alias
        );
        tracing::debug!("publisher_tracks: {:?}", self.tables.published_handlers);
        // The draft defines that the relay requires to send `PUBLISH_NAMESPACE` message to
        // any subscriber that has interests in the namespace
        // https://datatracker.ietf.org/doc/draft-ietf-moq-transport/

        // Convert DashMap<Namespace, DashSet<Uuid>> to DashMap<Uuid, DashSet<Namespace>>
        let combined = DashSet::new();
        self.tables
            .subscriber_namespaces
            .iter()
            .filter(|entry| entry.key().starts_with(&track_namespace))
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
                    .send_publish(track_namespace.clone(), track_name.clone(), track_alias)
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
        // TODO:
        // Send ok or error failed then close session.
        // forward: true case. prepare to accept stream/datagram before it returns the result.
        match handler.ok(128, FilterType::LatestObject).await {
            Ok(()) => self.tables.published_handlers.write().await.push(handler),
            Err(_) => tracing::error!("failed to accept publish. close session."),
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

        let pub_handlers = self.tables.published_handlers.read().await;
        let pub_handler = pub_handlers
            .iter()
            .find(|p| p.track_namespace() == track_namespace && p.track_name() == track_name);
        if let Some(pub_handler) = pub_handler {
            let option = SubscribeOption {
                subscriber_priority: handler.subscriber_priority(),
                group_order: handler.group_order(),
                forward: handler.forward(),
                start_location: handler.start_location(),
                end_group: handler.end_group(),
                filter_type: handler.filter_type(),
            };
            let subscription = pub_handler
                .subscribe(track_namespace.to_string(), track_name.to_string(), option)
                .await;
            if subscription.is_err() {
                let _ = handler.error(0, "failed to subscribe".to_string()).await;
            } else {
                let subscription = subscription.unwrap();
                match handler
                    .ok(subscription.track_alias(), subscription.expires(), false)
                    .await
                {
                    Ok(_) => {
                        tracing::info!("send `SUBSCRIBE_OK` ok");
                        let track_alias = subscription.track_alias();
                        let _ = self
                            .stream_handler
                            .bind_by_subscribe(subscription, handler.into_publication(track_alias))
                            .await;
                    }
                    Err(_) => {
                        tracing::error!("Failed to send `SUBSCRIBE_OK`. Session close.");
                    }
                }
            }
        } else {
            match handler
                .error(
                    0,
                    "Designated namespace and track name do not exist.".to_string(),
                )
                .await
            {
                Ok(_) => tracing::info!("send `SUBSCRIBE_ERROR` ok"),
                Err(e) => tracing::error!("Failed to send `SUBSCRIBE_ERROR`. Session close."),
            }
        }
    }
}
