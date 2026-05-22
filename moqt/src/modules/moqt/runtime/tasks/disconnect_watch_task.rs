use std::sync::Arc;

use tracing::{Instrument, Span};

use crate::{
    SessionEvent, TransportProtocol,
    modules::{
        moqt::domains::session_context::SessionContext,
        transport::transport_connection::TransportConnection,
    },
};

pub(crate) struct DisconnectWatchTask;

impl DisconnectWatchTask {
    pub(crate) fn run<T: TransportProtocol>(
        session_context: Arc<SessionContext<T>>,
        close_watcher_span: Span,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Connection Close Watcher")
            .spawn(
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
            .unwrap()
    }
}
