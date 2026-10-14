use std::sync::Arc;

use tracing::{Instrument, Span};

use crate::{
    modules::executor::{self, JoinHandle},
    modules::moqt::{
        data_plane::stream::stream_receiver::StreamReceiveError,
        domains::session_context::SessionContext,
        runtime::dispatch::{
            fetch_notifier::FetchNotifier, incoming_object::IncomingObject,
            subscription_notifier::SubscriptionNotifier,
        },
        version::IncomingStream,
    },
    modules::transport::transport_connection::BoxedReceiveStream,
};

pub(crate) struct UniStreamReceiveTask;

impl UniStreamReceiveTask {
    pub(crate) fn run(context: Arc<SessionContext>, stream_span: Span) -> JoinHandle {
        executor::spawn(
            "Uni Stream Receiver",
            async move {
                tracing::debug!("Uni Stream Receiver started");
                loop {
                    match context.transport_connection.accept_uni().await {
                        Ok(stream) => {
                            tracing::debug!("accepted incoming uni stream");
                            Self::on_stream_received(&context, stream).await;
                        }
                        Err(error) => {
                            tracing::debug!(%error, "Uni Stream Receiver stopped: the session ended");
                            break;
                        }
                    }
                }
            }
            .instrument(stream_span),
        )
    }

    #[tracing::instrument(level = "debug", name = "on_stream_received", skip_all)]
    async fn on_stream_received(context: &Arc<SessionContext>, stream: BoxedReceiveStream) -> bool {
        let incoming = match context.control.version().accept_uni_stream(stream).await {
            Ok(Some(incoming)) => incoming,
            Ok(None) => {
                tracing::debug!("Stream finished before its header");
                return false;
            }
            Err(error @ StreamReceiveError::Closed(_)) => {
                tracing::debug!(%error, "Stream ended before its header");
                return false;
            }
            Err(
                error @ (StreamReceiveError::Decode(_) | StreamReceiveError::ProtocolViolation(_)),
            ) => {
                tracing::error!(%error, "Failed to decode the stream header");
                return false;
            }
        };

        match incoming {
            IncomingStream::Subgroup(receiver) => {
                tracing::debug!(
                    track_alias = receiver.track_alias,
                    "received subgroup header"
                );
                SubscriptionNotifier::notify(
                    context,
                    receiver.track_alias,
                    IncomingObject::Subgroup(receiver),
                )
                .await;
            }
            IncomingStream::Fetch(receiver) => {
                FetchNotifier::notify(context, receiver.request_id, receiver).await;
            }
        }
        true
    }
}
