use std::{ops::ControlFlow, sync::Arc};

use bytes::BytesMut;
use tracing::{Instrument, Span};

use crate::{
    modules::executor::{self, JoinHandle},
    modules::moqt::{
        control_plane::constants::TerminationErrorCode,
        domains::connection::Connection,
        runtime::dispatch::{
            incoming_object::IncomingObject, subscription_notifier::SubscriptionNotifier,
        },
    },
};

pub(crate) struct DatagramReceiveTask;

impl DatagramReceiveTask {
    pub(crate) fn run(connection: Arc<Connection>, session_span: Span) -> JoinHandle {
        executor::spawn(
            "Datagram Receiver",
            async move {
                tracing::debug!("Datagram Receiver started");
                loop {
                    match connection.transport.receive_datagram().await {
                        Ok(mut data) => {
                            if Self::on_datagram_received(&connection, &mut data)
                                .await
                                .is_break()
                            {
                                break;
                            }
                        }
                        Err(error) => {
                            tracing::debug!(%error, "Datagram Receiver stopped: the session ended");
                            break;
                        }
                    }
                }
            }
            .instrument(session_span),
        )
    }

    #[tracing::instrument(level = "trace", name = "on_datagram_received", skip_all)]
    async fn on_datagram_received(connection: &Connection, data: &mut BytesMut) -> ControlFlow<()> {
        tracing::trace!(?data, "Received datagram");
        let (track_alias, datagram) = match connection.control.version().decode_datagram(data) {
            Ok(Some(decoded)) => decoded,
            Ok(None) => return ControlFlow::Continue(()),
            // QUIC protects a datagram's integrity, so one that does not decode
            // is the peer's protocol violation (draft-14 §10, §10.3.1;
            // draft-18 §11, §11.3.1).
            Err(error) => {
                tracing::error!(error = %format!("{error:#}"), "Failed to decode a datagram object");
                connection.close_with_error(
                    TerminationErrorCode::ProtocolViolation,
                    "malformed object datagram",
                );
                return ControlFlow::Break(());
            }
        };

        SubscriptionNotifier::notify(connection, track_alias, IncomingObject::Datagram(datagram))
            .await;
        ControlFlow::Continue(())
    }
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;

    use crate::{
        SessionEvent,
        modules::test_support::{DISCONNECT_TIMEOUT, DRAFTS, connected_sessions},
    };

    #[tokio::test]
    async fn a_datagram_of_an_unknown_type_closes_the_session() {
        for version in DRAFTS {
            // Arrange: neither draft defines datagram type 0x10
            let (client, server) = connected_sessions("datagram-unknown-type", version).await;
            let datagram = BytesMut::from(&[0x10, 0, 0, 0][..]);

            // Act
            client
                .subscriber()
                .session
                .connection()
                .transport
                .send_datagram(datagram)
                .unwrap();

            // Assert
            let event = tokio::time::timeout(DISCONNECT_TIMEOUT, server.receive_event())
                .await
                .expect("no session event")
                .unwrap();
            assert!(
                matches!(event, SessionEvent::ProtocolViolation()),
                "{event:?}"
            );
        }
    }
}
