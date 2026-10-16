use crate::modules::moqt::version::OutgoingResponse;
use std::sync::Arc;

use crate::{
    modules::{
        moqt::{
            control_plane::{
                control_messages::messages::{
                    fetch::Fetch, fetch_ok::FetchOk, parameters::location::Location,
                },
                error_codes::{RequestErrorCode, RequestKind},
                handler::response_guard::ResponseGuard,
            },
            domains::{connection::Connection, session_context::SessionContext},
        },
        transport::transport_send_stream::TransportSendError,
    },
    object::GroupOrder,
};

#[derive(Debug, Clone)]
pub struct FetchHandler {
    pub(crate) session_context: Arc<SessionContext>,
    pub request_id: u64,
    pub group_order: GroupOrder,
    pub fetch: Fetch,
    /// Sent as MAX_CACHE_DURATION (milliseconds, draft-14 §9.2.1.3) in the
    /// FETCH_OK.
    pub max_cache_duration: Option<u64>,
    guard: ResponseGuard,
}

impl FetchHandler {
    pub(crate) fn new(
        session_context: Arc<SessionContext>,
        connection: Arc<Connection>,
        fetch: Fetch,
    ) -> Self {
        let group_order = fetch.group_order;
        let guard = ResponseGuard::new(connection, fetch.request_id, RequestKind::Fetch);
        let request_id = guard.connection().logical_request_id(fetch.request_id);
        Self {
            session_context,
            request_id,
            group_order,
            fetch,
            max_cache_duration: None,
            guard,
        }
    }

    /// The FETCH's id on its connection, which its response stream carries.
    pub(crate) fn wire_request_id(&self) -> u64 {
        self.guard.request_id()
    }

    pub async fn ok(
        &self,
        end_of_track: bool,
        end_location: Location,
    ) -> Result<(), TransportSendError> {
        self.guard.mark_responded();
        let fetch_ok = FetchOk {
            request_id: self.guard.request_id(),
            group_order: self.group_order.delivered(),
            end_of_track,
            end_location,
            max_cache_duration: self.max_cache_duration,
        };
        self.guard
            .connection()
            .control
            .respond(OutgoingResponse::FetchOk(fetch_ok))
            .await
    }

    pub async fn error(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), TransportSendError> {
        self.guard.reject(code, reason_phrase).await
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        SessionEvent,
        fetch::FetchOption,
        modules::test_support::{connect_sessions, spawn_dual_server},
        object::{GroupOrder, Location},
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
