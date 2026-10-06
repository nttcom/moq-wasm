use std::sync::Arc;

use tracing::{Instrument, Span};

use crate::{
    SessionEvent,
    modules::executor::{self, JoinHandle},
    modules::moqt::domains::session_context::SessionContext,
};

pub(crate) struct DisconnectWatchTask;

impl DisconnectWatchTask {
    pub(crate) fn run(
        session_context: Arc<SessionContext>,
        close_watcher_span: Span,
    ) -> JoinHandle {
        executor::spawn(
            "Connection Close Watcher",
            async move {
                session_context.transport_connection.closed().await;

                if let Err(error) = session_context
                    .event_sender
                    .send(SessionEvent::Disconnected())
                {
                    tracing::warn!("failed to send disconnect event: {:?}", error);
                }
            }
            .instrument(close_watcher_span),
        )
    }
}
