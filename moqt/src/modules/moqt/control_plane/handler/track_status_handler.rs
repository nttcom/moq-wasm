use std::sync::Arc;

use crate::{
    FilterType, GroupOrder, TransportProtocol,
    modules::moqt::{
        control_plane::{
            control_messages::{
                control_message_type::ControlMessageType,
                messages::{
                    parameters::{
                        authorization_token::AuthorizationToken, content_exists::ContentExists,
                    },
                    request_error::RequestError,
                    subscribe::Subscribe,
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
pub struct TrackStatusHandler<T: TransportProtocol> {
    session_context: Arc<SessionContext<T>>,
    request_id: u64,
    track_namespace: String,
    track_name: String,
    subscriber_priority: u8,
    group_order: GroupOrder,
    forward: bool,
    filter_type: FilterType,
    authorization_tokens: Vec<AuthorizationToken>,
    guard: ResponseGuard<T>,
}

impl<T: TransportProtocol> TrackStatusHandler<T> {
    pub(crate) fn new(session_context: Arc<SessionContext<T>>, track_status: Subscribe) -> Self {
        let guard = ResponseGuard::new(
            session_context.clone(),
            track_status.request_id,
            ControlMessageType::TrackStatusError,
        );
        Self {
            session_context,
            request_id: track_status.request_id,
            track_namespace: track_status.track_namespace.join("/"),
            track_name: track_status.track_name,
            subscriber_priority: track_status.subscriber_priority,
            group_order: track_status.group_order,
            forward: track_status.forward,
            filter_type: track_status.filter_type,
            authorization_tokens: track_status.authorization_tokens,
            guard,
        }
    }

    /// The track's status is not inspected: TRACK_STATUS_OK is sent with
    /// Track Alias 0 (draft-14 §9.21) and Content Exists false.
    pub async fn ok(&self) -> Result<(), TransportSendError> {
        self.guard.mark_responded();
        let track_status_ok = SubscribeOk {
            request_id: self.request_id,
            track_alias: 0,
            expires: 0,
            group_order: self.group_order,
            content_exists: ContentExists::False,
            delivery_timeout: None,
            max_duration: None,
        };
        self.session_context
            .send_stream
            .send(ControlMessageType::TrackStatusOk, track_status_ok.encode())
            .await
    }

    pub async fn error(
        &self,
        error_code: u64,
        reason_phrase: String,
    ) -> Result<(), TransportSendError> {
        self.guard.mark_responded();
        let err = RequestError {
            request_id: self.request_id,
            error_code,
            reason_phrase,
        };
        self.session_context
            .send_stream
            .send(ControlMessageType::TrackStatusError, err.encode())
            .await
    }

    pub fn authorization_tokens(&self) -> &[AuthorizationToken] {
        &self.authorization_tokens
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    pub fn track_namespace(&self) -> &str {
        &self.track_namespace
    }

    pub fn track_name(&self) -> &str {
        &self.track_name
    }

    pub fn subscriber_priority(&self) -> u8 {
        self.subscriber_priority
    }

    pub fn group_order(&self) -> GroupOrder {
        self.group_order
    }

    pub fn forward(&self) -> bool {
        self.forward
    }

    pub fn filter_type(&self) -> FilterType {
        self.filter_type
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        ContentExists, DUAL, Session, SessionEvent,
        modules::{
            moqt::control_plane::control_messages::messages::parameters::authorization_token::AuthorizationToken,
            test_support::{connect_sessions, spawn_dual_server},
        },
        wire::{RequestError, TrackStatusOk},
    };

    use super::TrackStatusHandler;

    struct TrackStatusExchange {
        request: tokio::task::JoinHandle<anyhow::Result<TrackStatusOk>>,
        handler: TrackStatusHandler<DUAL>,
        _server: Session<DUAL>,
    }

    async fn track_status_exchange(name: &str) -> TrackStatusExchange {
        let (port, accept) = spawn_dual_server(name);
        let (client, server) = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
            .await
            .unwrap();
        let request = tokio::spawn(async move {
            client
                .subscriber()
                .track_status(
                    "app".to_string(),
                    "update_auth_token".to_string(),
                    vec![AuthorizationToken::use_value_utf8("jwt")],
                )
                .await
        });
        let SessionEvent::TrackStatus(handler) = server.receive_event().await.unwrap() else {
            panic!("expected TRACK_STATUS from the client");
        };
        TrackStatusExchange {
            request,
            handler,
            _server: server,
        }
    }

    fn request_error(result: anyhow::Result<TrackStatusOk>) -> RequestError {
        result
            .unwrap_err()
            .downcast::<RequestError>()
            .expect("expected a RequestError")
    }

    #[tokio::test]
    async fn ok_answers_with_track_alias_zero_and_no_content() {
        // Arrange
        let exchange = track_status_exchange("track-status-ok").await;

        // Act
        exchange.handler.ok().await.unwrap();

        // Assert
        let track_status_ok = exchange.request.await.unwrap().unwrap();
        assert_eq!(track_status_ok.track_alias, 0);
        assert_eq!(track_status_ok.content_exists, ContentExists::False);
    }

    #[tokio::test]
    async fn exposes_the_authorization_tokens_of_the_request() {
        // Arrange
        let exchange = track_status_exchange("track-status-tokens").await;

        // Act
        let tokens = exchange.handler.authorization_tokens().to_vec();

        // Assert
        assert_eq!(tokens, vec![AuthorizationToken::use_value_utf8("jwt")]);
    }

    #[tokio::test]
    async fn error_is_returned_to_the_requester_with_its_code() {
        // Arrange
        let exchange = track_status_exchange("track-status-error").await;

        // Act
        exchange
            .handler
            .error(0x1, "unauthorized".to_string())
            .await
            .unwrap();

        // Assert
        let error = request_error(exchange.request.await.unwrap());
        assert_eq!(error.error_code, 0x1);
        assert_eq!(error.reason_phrase, "unauthorized");
    }

    #[tokio::test]
    async fn dropping_the_handler_answers_not_supported() {
        // Arrange
        let exchange = track_status_exchange("track-status-dropped").await;

        // Act
        drop(exchange.handler);

        // Assert
        let error = request_error(exchange.request.await.unwrap());
        assert_eq!(error.error_code, 0x3);
    }
}
