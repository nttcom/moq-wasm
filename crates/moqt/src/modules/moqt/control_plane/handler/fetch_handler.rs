use std::sync::Arc;

use crate::{
    GroupOrder,
    modules::{
        moqt::{
            control_plane::{
                control_messages::{
                    control_message_type::ControlMessageType,
                    messages::{
                        fetch::Fetch, fetch_ok::FetchOk, parameters::location::Location,
                        request_error::RequestError,
                    },
                },
                handler::response_guard::ResponseGuard,
            },
            domains::session_context::SessionContext,
        },
        transport::transport_send_stream::TransportSendError,
    },
};

#[derive(Debug, Clone)]
pub struct FetchHandler {
    session_context: Arc<SessionContext>,
    pub request_id: u64,
    pub group_order: GroupOrder,
    pub fetch: Fetch,
    guard: ResponseGuard,
}

impl FetchHandler {
    pub(crate) fn new(session_context: Arc<SessionContext>, fetch: Fetch) -> Self {
        let request_id = fetch.request_id;
        let group_order = fetch.group_order;
        let guard = ResponseGuard::new(
            session_context.clone(),
            request_id,
            ControlMessageType::FetchError,
        );
        Self {
            session_context,
            request_id,
            group_order,
            fetch,
            guard,
        }
    }

    pub async fn ok(
        &self,
        end_of_track: bool,
        end_location: Location,
    ) -> Result<(), TransportSendError> {
        self.guard.mark_responded();
        let fetch_ok = FetchOk {
            request_id: self.request_id,
            group_order: self.group_order.delivered(),
            end_of_track,
            end_location,
            max_cache_duration: None,
        };
        self.session_context
            .send_stream
            .send(ControlMessageType::FetchOk, fetch_ok.encode())
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
            .send(ControlMessageType::FetchError, err.encode())
            .await
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        FetchOption, GroupOrder, Location, SessionEvent,
        modules::test_support::{connect_sessions, spawn_dual_server},
    };

    const START: Location = Location {
        group_id: 0,
        object_id: 0,
    };
    const END: Location = Location {
        group_id: 1,
        object_id: 0,
    };

    #[tokio::test]
    async fn fetch_ok_states_ascending_when_the_subscriber_leaves_the_order_to_the_publisher() {
        // Arrange
        let (port, accept) = spawn_dual_server("fetch-handler-group-order");
        let (client, server) = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
            .await
            .unwrap();
        let mut subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .fetch(
                    "ns".to_string(),
                    "track".to_string(),
                    START,
                    END,
                    FetchOption {
                        group_order: GroupOrder::Publisher,
                        ..FetchOption::default()
                    },
                )
                .await
        });
        let SessionEvent::Fetch(handler) = server.receive_event().await.unwrap() else {
            panic!("expected FETCH from the client");
        };

        // Act
        handler.ok(false, END).await.unwrap();

        // Assert
        let fetch_handle = request.await.unwrap().unwrap();
        assert_eq!(fetch_handle.group_order, GroupOrder::Ascending);
    }
}
