use crate::modules::moqt::control_plane::control_messages::messages::parameters::trace_context::TraceContext;
use crate::modules::moqt::version::{OutgoingResponse, RequestSummary};
use std::sync::Arc;

use crate::{
    modules::{
        moqt::{
            control_plane::{
                constants::TerminationErrorCode,
                control_messages::messages::{
                    fetch::{Fetch, FetchParams},
                    fetch_ok::FetchOk,
                    parameters::location::Location,
                },
                error_codes::RequestErrorCode,
                handler::response_guard::ResponseGuard,
            },
            domains::{
                connection::Connection, fetch_range::JoiningStart, session_context::SessionContext,
            },
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
    /// The TRACE_CONTEXT the request carried, if the peer sent a valid one.
    pub fn trace_context(&self) -> Option<&TraceContext> {
        self.fetch.trace_context.as_ref()
    }

    pub(crate) fn new(
        session_context: Arc<SessionContext>,
        connection: Arc<Connection>,
        fetch: Fetch,
    ) -> Self {
        let group_order = fetch.group_order;
        let guard = ResponseGuard::new(connection, RequestSummary::of_fetch(&fetch));
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

    pub async fn ok(
        &self,
        end_of_track: bool,
        end_location: Location,
    ) -> Result<(), TransportSendError> {
        let fetch_ok = FetchOk {
            request_id: self.guard.request_id(),
            group_order: self.group_order.delivered(),
            end_of_track,
            end_location,
            max_cache_duration: self.max_cache_duration,
        };
        self.guard.accept(OutgoingResponse::FetchOk(fetch_ok)).await
    }

    pub async fn error(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), TransportSendError> {
        self.guard.reject(code, reason_phrase).await
    }

    pub(crate) fn logical_request_id(&self, wire_id: u64) -> u64 {
        self.guard.connection().logical_request_id(wire_id)
    }

    /// The SUBSCRIBE a Joining Fetch joins, by the request id
    /// `SubscribeHandler::request_id` gave it, with the fetch's Joining Start
    /// (draft-14 §9.16.2); `None` for a Standalone Fetch.
    pub fn joining(&self) -> Option<(u64, JoiningStart)> {
        let (joining_request_id, joining_start) = match &self.fetch.fetch_params {
            FetchParams::Standalone { .. } => return None,
            FetchParams::RelativeJoining {
                joining_request_id,
                joining_start,
            } => (*joining_request_id, JoiningStart::Relative(*joining_start)),
            FetchParams::AbsoluteJoining {
                joining_request_id,
                joining_start,
            } => (*joining_request_id, JoiningStart::Absolute(*joining_start)),
        };
        Some((self.logical_request_id(joining_request_id), joining_start))
    }

    /// Closes the session with PROTOCOL_VIOLATION instead of answering. Call it
    /// for a Joining Fetch of a subscription without the Largest Object filter
    /// (draft-14 §9.16.2).
    pub fn close_for_protocol_violation(&self, reason: &str) {
        self.guard.mark_responded();
        self.session_context.close_connection(
            self.guard.connection(),
            TerminationErrorCode::ProtocolViolation,
            reason,
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        SessionEvent,
        fetch::{FetchOption, JoiningStart},
        modules::test_support::{connect_sessions, spawn_native_server},
        object::{ContentExists, GroupOrder, Location},
        subscribe::{FilterType, SubscribeOption},
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
        let (port, accept) = spawn_native_server("fetch-handler-group-order");
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

    #[tokio::test]
    async fn a_joining_fetch_names_the_request_id_its_subscribe_handler_gave() {
        // Arrange
        let (port, accept) = spawn_native_server("fetch-handler-joining");
        let (client, server) = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
            .await
            .unwrap();
        let mut subscriber = client.subscriber();
        let subscribing = tokio::spawn(async move {
            let subscription = subscriber
                .subscribe(
                    "ns".to_string(),
                    "track".to_string(),
                    SubscribeOption {
                        filter_type: FilterType::LargestObject,
                        ..SubscribeOption::default()
                    },
                )
                .await
                .unwrap();
            let _ = subscriber
                .fetch_relative_joining(&subscription, 2, FetchOption::default())
                .await;
        });
        let SessionEvent::Subscribe(subscribe) = server.receive_event().await.unwrap() else {
            panic!("expected SUBSCRIBE from the client");
        };
        subscribe
            .ok(0, ContentExists::True { location: END })
            .await
            .unwrap();

        // Act
        let SessionEvent::Fetch(fetch) = server.receive_event().await.unwrap() else {
            panic!("expected FETCH from the client");
        };

        // Assert
        assert_eq!(
            fetch.joining(),
            Some((subscribe.request_id(), JoiningStart::Relative(2)))
        );
        fetch.ok(true, END).await.unwrap();
        subscribing.await.unwrap();
    }
}
