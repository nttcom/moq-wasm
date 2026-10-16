use std::sync::Arc;

use tracing::{Instrument, Span};

use crate::{
    SessionEvent,
    modules::executor::{self, JoinHandle},
    modules::moqt::domains::connection::Connection,
};

pub(crate) struct DisconnectWatchTask;

impl DisconnectWatchTask {
    pub(crate) fn run(
        connection: Arc<Connection>,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
        close_watcher_span: Span,
    ) -> JoinHandle {
        executor::spawn(
            "Connection Close Watcher",
            async move {
                connection.transport.closed().await;

                // Nobody reads the events of a connection a migration retired.
                if event_sender.send(SessionEvent::Disconnected()).is_err() {
                    tracing::debug!(
                        "disconnect event not delivered: nobody reads the connection's events"
                    );
                }
            }
            .instrument(close_watcher_span),
        )
    }
}
