use std::sync::Arc;

use crate::{
    FilterType, GroupOrder, RequestErrorCode, RequestKind, SubscriberInitiatedSubscription,
    Subscription, TransportProtocol,
    modules::moqt::{
        control_plane::{
            control_messages::{
                control_message_type::ControlMessageType,
                messages::{
                    parameters::content_exists::ContentExists, subscribe::Subscribe,
                    subscribe_ok::SubscribeOk,
                },
            },
            handler::response_guard::ResponseGuard,
        },
        domains::session_context::SessionContext,
    },
    modules::transport::transport_send_stream::TransportSendError,
};

#[derive(Debug, Clone)]
pub struct SubscribeHandler<T: TransportProtocol> {
    session_context: Arc<SessionContext<T>>,
    request_id: u64,
    pub track_namespace: String,
    pub track_namespace_tuple: Vec<String>,
    pub track_name: String,
    pub subscriber_priority: u8,
    pub group_order: GroupOrder,
    pub forward: bool,
    pub filter_type: FilterType,
    pub max_cache_duration: Option<u64>,
    pub delivery_timeout: Option<u64>,
    guard: ResponseGuard<T>,
}

impl<T: TransportProtocol> SubscribeHandler<T> {
    pub(crate) fn new(
        session_context: Arc<SessionContext<T>>,
        subscribe_message: Subscribe,
    ) -> Self {
        let guard = ResponseGuard::new(
            session_context.clone(),
            subscribe_message.request_id,
            RequestKind::Subscribe,
        );
        Self {
            session_context,
            guard,
            request_id: subscribe_message.request_id,
            track_namespace: subscribe_message.track_namespace.join("/"),
            track_namespace_tuple: subscribe_message.track_namespace,
            track_name: subscribe_message.track_name,
            subscriber_priority: subscribe_message.subscriber_priority,
            group_order: subscribe_message.group_order,
            forward: subscribe_message.forward,
            filter_type: subscribe_message.filter_type,
            max_cache_duration: None,
            delivery_timeout: None,
        }
    }

    pub async fn ok(
        &self,
        expires: u64,
        content_exists: ContentExists,
    ) -> Result<u64, TransportSendError> {
        let track_alias = self.allocate_track_alias();
        self.ok_with_track_alias(track_alias, expires, content_exists)
            .await?;
        Ok(track_alias)
    }

    pub async fn ok_with_track_alias(
        &self,
        track_alias: u64,
        expires: u64,
        content_exists: ContentExists,
    ) -> Result<(), TransportSendError> {
        self.guard.mark_responded();
        let subscribe_ok = SubscribeOk {
            request_id: self.request_id,
            track_alias,
            expires,
            group_order: self.group_order.delivered(),
            content_exists,
            delivery_timeout: self.delivery_timeout,
            max_duration: self.max_cache_duration,
        };
        self.session_context
            .send_stream
            .send(ControlMessageType::SubscribeOk, subscribe_ok.encode())
            .await?;
        Ok(())
    }

    pub fn allocate_track_alias(&self) -> u64 {
        self.session_context.get_track_alias()
    }

    pub async fn error(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), TransportSendError> {
        self.guard.reject(code, reason_phrase).await
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    /// Builds the subscription this handler represents. Receiving a SUBSCRIBE
    /// makes it subscriber-initiated.
    pub fn into_subscription(&self, track_alias: u64) -> Subscription {
        Subscription::SubscriberInitiated(SubscriberInitiatedSubscription::from_subscribe_handler(
            track_alias,
            self,
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::SubscribeOption;
    use crate::{
        ContentExists, GroupOrder, SessionEvent,
        modules::test_support::{connect_sessions, spawn_dual_server},
    };

    #[tokio::test]
    async fn subscribe_ok_states_ascending_when_the_subscriber_leaves_the_order_to_the_publisher() {
        // Arrange
        let (port, accept) = spawn_dual_server("subscribe-handler-group-order");
        let (client, server) = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
            .await
            .unwrap();
        let request = tokio::spawn(async move {
            client
                .subscriber()
                .subscribe(
                    "ns".to_string(),
                    "track".to_string(),
                    SubscribeOption {
                        group_order: GroupOrder::Publisher,
                        ..SubscribeOption::default()
                    },
                )
                .await
        });
        let SessionEvent::Subscribe(handler) = server.receive_event().await.unwrap() else {
            panic!("expected SUBSCRIBE from the client");
        };

        // Act
        handler.ok(0, ContentExists::False).await.unwrap();

        // Assert
        let subscription = request.await.unwrap().unwrap();
        assert_eq!(subscription.group_order(), GroupOrder::Ascending);
    }

    #[tokio::test]
    async fn exposes_track_namespace_as_tuple_and_joined_string() {
        // Arrange
        let (port, accept) = spawn_dual_server("subscribe-handler-namespace");
        let (client, server) = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
            .await
            .unwrap();
        let request = tokio::spawn(async move {
            client
                .subscriber()
                .subscribe(
                    "a/b/c".to_string(),
                    "track".to_string(),
                    SubscribeOption::default(),
                )
                .await
        });

        // Act
        let SessionEvent::Subscribe(handler) = server.receive_event().await.unwrap() else {
            panic!("expected SUBSCRIBE from the client");
        };

        // Assert
        assert_eq!(handler.track_namespace_tuple, vec!["a", "b", "c"]);
        assert_eq!(handler.track_namespace, "a/b/c");
        request.abort();
    }
}
