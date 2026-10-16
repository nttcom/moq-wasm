use crate::modules::moqt::version::OutgoingResponse;
use std::sync::Arc;

use crate::{
    PublisherInitiatedSubscription, Subscription,
    modules::moqt::{
        control_plane::{
            control_messages::messages::{
                parameters::content_exists::ContentExists, publish::Publish, publish_ok::PublishOk,
            },
            error_codes::{RequestErrorCode, RequestKind},
            handler::response_guard::ResponseGuard,
        },
        domains::{connection::Connection, session_context::SessionContext},
    },
    modules::transport::transport_send_stream::TransportSendError,
    object::GroupOrder,
    subscribe::FilterType,
};

#[derive(Debug, Clone)]
pub struct PublishHandler {
    session_context: Arc<SessionContext>,
    pub request_id: u64,
    pub track_namespace: String,
    pub track_namespace_tuple: Vec<String>,
    pub track_name: String,
    pub track_alias: u64,
    pub group_order: GroupOrder,
    pub content_exists: ContentExists,
    pub forward: bool,
    /// MAX_CACHE_DURATION in milliseconds (draft-14 §9.2.1.3) the PUBLISH
    /// carried.
    pub max_cache_duration: Option<u64>,
    pub delivery_timeout: Option<u64>,
    guard: ResponseGuard,
}

impl PublishHandler {
    pub(crate) fn new(
        session_context: Arc<SessionContext>,
        connection: Arc<Connection>,
        publish_message: Publish,
    ) -> Self {
        let guard =
            ResponseGuard::new(connection, publish_message.request_id, RequestKind::Publish);
        Self {
            session_context,
            guard,
            request_id: publish_message.request_id,
            track_namespace: publish_message.track_namespace_tuple.join("/"),
            track_namespace_tuple: publish_message.track_namespace_tuple,
            track_name: publish_message.track_name,
            track_alias: publish_message.track_alias,
            group_order: publish_message.group_order,
            content_exists: publish_message.content_exists,
            forward: publish_message.forward,
            max_cache_duration: publish_message.max_duration,
            delivery_timeout: None,
        }
    }

    pub async fn ok(
        &self,
        subscriber_priority: u8,
        filter_type: FilterType,
        expires: u64,
    ) -> Result<Subscription, TransportSendError> {
        self.guard.mark_responded();
        let publish_ok = PublishOk {
            request_id: self.request_id,
            forward: self.forward,
            subscriber_priority,
            group_order: self.group_order,
            filter_type,
            delivery_timeout: self.delivery_timeout,
        };
        self.guard
            .connection()
            .control
            .respond(OutgoingResponse::PublishOk(publish_ok))
            .await?;

        let _ = expires;
        Ok(Subscription::PublisherInitiated(
            PublisherInitiatedSubscription {
                request_id: self.request_id,
                track_namespace: self.track_namespace.clone(),
                track_name: self.track_name.clone(),
                track_alias: self.track_alias,
                group_order: self.group_order,
                content_exists: self.content_exists,
                subscriber_priority,
                forward: self.forward,
                filter_type,
                delivery_timeout: self.delivery_timeout,
                max_cache_duration: self.max_cache_duration,
            },
        ))
    }

    pub async fn accept_data_receiver(&self) {
        if let Err(code) = self
            .session_context
            .register_data_receiver(self.guard.connection(), self.request_id, self.track_alias)
            .await
        {
            // The track alias is already bound to another active subscription.
            // draft-14 §9.8: close the session with DUPLICATE_TRACK_ALIAS.
            tracing::error!(
                track_alias = self.track_alias,
                "PUBLISH reused an in-use track alias; closing session"
            );
            self.session_context.close_connection(
                self.guard.connection(),
                code,
                "PUBLISH reused an in-use track alias",
            );
            return;
        }
        tracing::info!(
            track_alias = self.track_alias,
            "publish handler registered incoming object receiver"
        );
    }

    pub async fn error(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), TransportSendError> {
        self.session_context
            .cancel_unclaimed_track_alias(self.guard.connection(), self.track_alias)
            .await;
        self.guard.reject(code, reason_phrase).await
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::error::TryRecvError;

    use crate::RequestErrorCode;
    use crate::publish::PublishOption;
    use crate::subscribe::FilterType;
    use crate::{
        SessionEvent,
        modules::{
            moqt::domains::connection::IncomingObjectNotification,
            test_support::{
                notify_datagram, register_and_take_data_receiver, spawn_connected_dual_sessions,
            },
        },
    };

    const OTHER_REQUEST_ID: u64 = 99;

    #[tokio::test]
    async fn publish_hands_its_max_cache_duration_to_both_subscriptions() {
        // Arrange
        let (client, server) =
            spawn_connected_dual_sessions("publish-handler-max-cache-duration").await;
        let request = tokio::spawn(async move {
            client
                .publisher()
                .publish(
                    "ns".to_string(),
                    "track".to_string(),
                    PublishOption {
                        max_cache_duration: Some(2_000),
                        ..PublishOption::default()
                    },
                )
                .await
        });
        let SessionEvent::Publish(handler) = server.receive_event().await.unwrap() else {
            panic!("expected PUBLISH from the client");
        };

        // Act
        let accepted = handler.ok(128, FilterType::LargestObject, 0).await.unwrap();

        // Assert
        assert_eq!(handler.max_cache_duration, Some(2_000));
        assert_eq!(accepted.max_cache_duration(), Some(2_000));
        let published = request.await.unwrap().unwrap();
        assert_eq!(published.max_cache_duration(), Some(2_000));
    }

    #[tokio::test]
    async fn exposes_track_namespace_as_tuple_and_joined_string() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("publish-handler-namespace").await;
        let request = tokio::spawn(async move {
            client
                .publisher()
                .publish(
                    "a/b/c".to_string(),
                    "track".to_string(),
                    PublishOption::default(),
                )
                .await
        });

        // Act
        let SessionEvent::Publish(handler) = server.receive_event().await.unwrap() else {
            panic!("expected PUBLISH from the client");
        };

        // Assert
        assert_eq!(handler.track_namespace_tuple, vec!["a", "b", "c"]);
        assert_eq!(handler.track_namespace, "a/b/c");
        request.abort();
    }

    #[tokio::test]
    async fn publish_error_drops_the_objects_buffered_for_its_track_alias() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("publish-handler-error-buffer").await;
        let request = tokio::spawn(async move {
            client
                .publisher()
                .publish(
                    "ns".to_string(),
                    "track".to_string(),
                    PublishOption::default(),
                )
                .await
        });
        let SessionEvent::Publish(handler) = server.receive_event().await.unwrap() else {
            panic!("expected PUBLISH from the client");
        };
        let context = server.subscriber().session;
        notify_datagram(&context, handler.track_alias).await;

        // Act
        handler
            .error(RequestErrorCode::InternalError, "uninterested".to_string())
            .await
            .unwrap();
        let notification = notify_datagram(&context, handler.track_alias).await;

        // Assert
        assert!(
            matches!(notification, IncomingObjectNotification::Discarded),
            "{notification:?}"
        );
        let mut receiver =
            register_and_take_data_receiver(&context, OTHER_REQUEST_ID, handler.track_alias).await;
        let received = receiver.try_recv();
        assert!(
            matches!(&received, Err(TryRecvError::Empty)),
            "{received:?}"
        );
        request.abort();
    }
}
