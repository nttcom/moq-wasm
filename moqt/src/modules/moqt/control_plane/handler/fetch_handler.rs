use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::{
    GroupOrder, TransportProtocol,
    modules::{
        moqt::{
            control_plane::control_messages::{
                control_message_type::ControlMessageType,
                messages::{
                    fetch::Fetch, fetch_ok::FetchOk, parameters::location::Location,
                    request_error::RequestError,
                },
            },
            domains::session_context::SessionContext,
        },
        transport::transport_send_stream::TransportSendError,
    },
};

/// FETCH_ERROR code for endpoints that do not implement FETCH (draft-14 §9.18).
const FETCH_ERROR_NOT_SUPPORTED: u64 = 0x3;

#[derive(Debug, Clone)]
pub struct FetchHandler<T: TransportProtocol> {
    session_context: Arc<SessionContext<T>>,
    pub request_id: u64,
    pub group_order: GroupOrder,
    pub fetch: Fetch,
    responded: Arc<AtomicBool>,
}

impl<T: TransportProtocol> FetchHandler<T> {
    pub(crate) fn new(session_context: Arc<SessionContext<T>>, fetch: Fetch) -> Self {
        let request_id = fetch.request_id;
        let group_order = fetch.group_order;
        Self {
            session_context,
            request_id,
            group_order,
            fetch,
            responded: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn ok(
        &self,
        end_of_track: bool,
        end_location: Location,
    ) -> Result<(), TransportSendError> {
        self.responded.store(true, Ordering::Relaxed);
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
        self.responded.store(true, Ordering::Relaxed);
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

impl<T: TransportProtocol> Drop for FetchHandler<T> {
    fn drop(&mut self) {
        // An application that ignores SessionEvent::Fetch drops the handler
        // without responding, which would leave the requester waiting for its
        // control timeout. Auto-answer NOT_SUPPORTED so the failure is prompt
        // and the peer's session is never at risk. strong_count == 1 limits
        // this to the last clone; a single fire-and-forget send needs no
        // owned JoinHandle.
        if self.responded.load(Ordering::Relaxed) || Arc::strong_count(&self.responded) > 1 {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let session_context = self.session_context.clone();
        let request_id = self.request_id;
        runtime.spawn(async move {
            let err = RequestError {
                request_id,
                error_code: FETCH_ERROR_NOT_SUPPORTED,
                reason_phrase: "fetch not handled by application".to_string(),
            };
            if let Err(error) = session_context
                .send_stream
                .send(ControlMessageType::FetchError, err.encode())
                .await
            {
                tracing::warn!(?error, request_id, "failed to auto-reject unhandled fetch");
            }
        });
    }
}
