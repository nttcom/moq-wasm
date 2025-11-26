use std::sync::Arc;

use crate::modules::{
    core::handler::{
        publish::PublishHandler, publish_namespace::PublishNamespaceHandler,
        subscribe::SubscribeHandler, subscribe_namespace::SubscribeNamespaceHandler,
    },
    enums::{ContentExists, FilterType, Location},
    event_resolver::stream_binder::StreamBinder,
    repositories::session_repository::SessionRepository,
    sequences::{
        notifier::Notifier,
        tables::{hashmap_table::HashMapTable, table::Table},
    },
    types::SessionId,
};

pub(crate) struct SequenceHandler {
    stream_handler: StreamBinder,
    notifier: Notifier,
    table: Box<dyn Table>,
}

impl SequenceHandler {
    pub(crate) fn new(session_repo: Arc<tokio::sync::Mutex<SessionRepository>>) -> Self {
        Self {
            stream_handler: StreamBinder::new(),
            notifier: Notifier {
                repository: session_repo,
            },
            table: Box::new(HashMapTable::new()),
        }
    }

    pub(crate) async fn publish_namespace(
        &mut self,
        session_id: SessionId,
        handler: Box<dyn PublishNamespaceHandler>,
    ) {
        tracing::info!("SequenceHandler::publish namespace: {}", session_id);
        let track_namespace = handler.track_namespace();

        if !self
            .table
            .register_publish_namespace(session_id, track_namespace.to_string())
        {
            // TODO: Session close.
            tracing::error!("Failed to register publish namespace");
            match handler
                .error(0, "Failed to register publish namespace".to_string())
                .await
            {
                Ok(_) => tracing::info!("send `PUBLISH_NAMESPACE_ERROR` ok"),
                Err(_) => {
                    tracing::error!("Failed to send `PUBLISH_NAMESPACE_ERROR`. Session close.")
                }
            }
            return;
        }
        // The draft defines that the relay requires to send `PUBLISH_NAMESPACE` message to
        // any subscriber that has interests in the namespace
        // https://datatracker.ietf.org/doc/draft-ietf-moq-transport/

        // Convert DashMap<Namespace, DashSet<Uuid>> to DashMap<Uuid, DashSet<Namespace>>
        let combined = self.table.get_namespace_subscribers(track_namespace);
        tracing::debug!("The namespace are subscribed by: {:?}", combined);
        for session_id in combined {
            if self
                .notifier
                .publish_namespace(session_id, track_namespace.to_string())
                .await
            {
                tracing::info!(
                    "Sent publish namespace '{}' to {}",
                    track_namespace,
                    session_id
                )
            } else {
                tracing::warn!("Failed to send publish namespace: {}", session_id);
            }
        }

        match handler.ok().await {
            Ok(_) => tracing::info!("OK"),
            Err(e) => {
                tracing::error!("Publish Namespace Error: {:?}", e);
                return;
            }
        }
        tracing::info!("SequenceHandler::publish namespace: {} DONE", session_id);
    }

    pub(crate) async fn subscribe_namespace(
        &mut self,
        session_id: SessionId,
        handler: Box<dyn SubscribeNamespaceHandler>,
    ) {
        tracing::info!("SequenceHandler::subscribe namespace: {}", session_id);
        let track_namespace_prefix = handler.track_namespace_prefix();
        tracing::info!(
            "New namespace prefix '{}' has been subscribed.",
            track_namespace_prefix
        );
        self.table
            .register_subscribe_namespace(session_id, track_namespace_prefix.to_string());

        let filtered = self.table.get_subscribers(track_namespace_prefix).await;

        tracing::debug!("The namespace prefix are subscribed by: {:?}", filtered);

        for (track_namespace, publish_values) in filtered {
            if let (Some(track_name), Some(track_alias)) = publish_values {
                if self
                    .notifier
                    .publish(
                        session_id,
                        track_namespace.clone(),
                        track_name.clone(),
                        track_alias,
                    )
                    .await
                {
                    tracing::info!("Sent publish '{}' to {}", track_namespace, session_id)
                } else {
                    tracing::warn!("Failed to send publish: {}", session_id);
                }
            } else if self
                .notifier
                .publish_namespace(session_id, track_namespace.clone())
                .await
            {
                tracing::info!("Sent publish '{}' to {}", track_namespace, session_id);
            } else {
                tracing::error!("Failed to send publish namespace");
            }
        }

        tracing::info!("subscribe ok will be sent.");
        match handler.ok().await {
            Ok(_) => tracing::info!("OK"),
            Err(e) => {
                tracing::error!("Subscribe Namespace Error: {:?}", e);
                return;
            }
        }
        tracing::info!("SequenceHandler::subscribe namespace: {} DONE", session_id);
    }

    pub(crate) async fn publish(&self, session_id: SessionId, handler: Box<dyn PublishHandler>) {
        tracing::info!("SequenceHandler::publish: {}", session_id);
        let track_namespace = handler.track_namespace().to_string();
        let track_name = handler.track_name().to_string();
        let track_alias = handler.track_alias();

        let full_track_namespace = format!("{}:{}", track_namespace, track_name);

        tracing::info!(
            "New track '{}' (alias '{}') has been published.",
            full_track_namespace,
            track_alias
        );
        // The draft defines that the relay requires to send `PUBLISH_NAMESPACE` message to
        // any subscriber that has interests in the namespace
        // https://datatracker.ietf.org/doc/draft-ietf-moq-transport/

        // Convert DashMap<Namespace, DashSet<Uuid>> to DashMap<Uuid, DashSet<Namespace>>
        let combined = self.table.get_namespace_subscribers(&track_namespace);
        tracing::debug!("The namespace are subscribed by: {:?}", combined);
        for session_id in combined {
            if self
                .notifier
                .publish(
                    session_id,
                    track_namespace.clone(),
                    track_name.clone(),
                    track_alias,
                )
                .await
            {
                tracing::info!("Sent publish '{}' to {}", track_namespace, session_id);
            } else {
                tracing::warn!("Failed to send publish: {}", session_id);
            }
        }
        // TODO:
        // Send ok or error failed then close session.
        // forward: true case. prepare to accept stream/datagram before it returns the result.
        match handler.ok(128, FilterType::LatestObject).await {
            Ok(()) => self.table.register_publish(Arc::from(handler)).await,
            Err(_) => tracing::error!("failed to accept publish. close session."),
        }
        tracing::info!("SequenceHandler::publish: {} DONE", session_id);
    }

    pub(crate) async fn subscribe(
        &self,
        session_id: SessionId,
        handler: Box<dyn SubscribeHandler>,
    ) {
        tracing::info!("SequenceHandler::subscribe: {}", session_id);
        let track_namespace = handler.track_namespace();
        let track_name = handler.track_name();
        let full_track_namespace = format!("{}:{}", track_namespace, track_name);
        tracing::debug!("New track '{}' has been subscribed.", full_track_namespace,);

        let pub_handler = self
            .table
            .find_publish_handler_with(handler.track_namespace(), handler.track_name())
            .await;
        if let Some(pub_handler) = pub_handler {
            tracing::info!(
                "publisher found. {}/{} (alias {})",
                pub_handler.track_namespace(),
                pub_handler.track_name(),
                pub_handler.track_alias()
            );
            let subscription = pub_handler.into_subscription(0);
            match handler
                .ok(
                    subscription.track_alias(),
                    subscription.expires(),
                    ContentExists::True {
                        location: Location {
                            group_id: 0,
                            object_id: 0,
                        },
                    },
                )
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
        } else if let Some(session_id) = self.table.get_publish_namespace(track_namespace) {
            if let Ok(subscription) = self
                .notifier
                .subscribe(
                    session_id,
                    track_namespace.to_string(),
                    track_name.to_string(),
                )
                .await
            {
                tracing::info!("send `SUBSCRIBE_OK` ok");
            } else {
                tracing::warn!("Failed to send `SUBSCRIBE_OK`. Session close.");
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
                Err(_) => tracing::error!("Failed to send `SUBSCRIBE_ERROR`. Session close."),
            }
        }
        tracing::info!("SequenceHandler::subscribe: {} DONE", session_id);
    }
}
