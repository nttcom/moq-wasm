use std::sync::Arc;

use crate::{
    GroupOrder, TransportProtocol,
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
pub struct FetchHandler<T: TransportProtocol> {
    session_context: Arc<SessionContext<T>>,
    pub request_id: u64,
    pub group_order: GroupOrder,
    pub fetch: Fetch,
    guard: ResponseGuard<T>,
}

impl<T: TransportProtocol> FetchHandler<T> {
    pub(crate) fn new(session_context: Arc<SessionContext<T>>, fetch: Fetch) -> Self {
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
            group_order: self.group_order,
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
