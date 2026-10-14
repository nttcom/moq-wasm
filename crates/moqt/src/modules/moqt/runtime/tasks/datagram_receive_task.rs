use std::sync::Arc;

use bytes::BytesMut;
use tracing::{Instrument, Span};

use crate::{
    modules::executor::{self, JoinHandle},
    modules::moqt::{
        domains::session_context::SessionContext,
        runtime::dispatch::{
            incoming_object::IncomingObject, subscription_notifier::SubscriptionNotifier,
        },
    },
};

pub(crate) struct DatagramReceiveTask;

impl DatagramReceiveTask {
    pub(crate) fn run(context: Arc<SessionContext>, datagram_span: Span) -> JoinHandle {
        executor::spawn(
            "Datagram Receiver",
            async move {
                tracing::debug!("Datagram Receiver started");
                loop {
                    match context.transport_connection.receive_datagram().await {
                        Ok(mut data) => {
                            Self::on_datagram_received(&context, &mut data).await;
                        }
                        Err(error) => {
                            tracing::debug!(%error, "Datagram Receiver stopped: the session ended");
                            break;
                        }
                    }
                }
            }
            .instrument(datagram_span),
        )
    }

    #[tracing::instrument(level = "trace", name = "on_datagram_received", skip_all)]
    async fn on_datagram_received(context: &Arc<SessionContext>, data: &mut BytesMut) -> bool {
        tracing::trace!(?data, "Received datagram");
        let (track_alias, object) = match context.control.version().decode_datagram(data) {
            Ok(decoded) => decoded,
            Err(error) => {
                tracing::error!(error = %format!("{error:#}"), "Failed to decode a datagram object");
                return false;
            }
        };

        SubscriptionNotifier::notify(context, track_alias, IncomingObject::Datagram(object)).await;
        true
    }
}
