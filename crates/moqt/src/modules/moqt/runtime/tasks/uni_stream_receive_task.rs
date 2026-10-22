use std::sync::Arc;

use futures::{StreamExt, stream};
use tracing::{Instrument, Span};

use crate::{
    modules::executor::{self, JoinHandle},
    modules::moqt::{
        data_plane::stream::stream_receiver::StreamReceiveError,
        domains::connection::Connection,
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
    pub(crate) fn run(connection: Arc<Connection>, stream_span: Span) -> JoinHandle {
        executor::spawn(
            "Uni Stream Receiver",
            async move {
                tracing::debug!("Uni Stream Receiver started");
                let accepted_streams = stream::unfold(&connection, |connection| async move {
                    match connection.transport.accept_uni().await {
                        Ok(stream) => {
                            tracing::debug!("accepted incoming uni stream");
                            Some((stream, connection))
                        }
                        Err(error) => {
                            tracing::debug!(%error, "Uni Stream Receiver stopped: the session ended");
                            None
                        }
                    }
                });
                // A peer can send a stream's header after data on higher-priority
                // streams, so a missing header must not block the streams accepted
                // after it. accept_uni is not cancel-safe on the browser transport, so
                // its future must never be dropped before it resolves, as a select!
                // loop would do.
                accepted_streams
                    .for_each_concurrent(None, |stream| async {
                        Self::on_stream_received(&connection, stream).await;
                    })
                    .await;
            }
            .instrument(stream_span),
        )
    }

    #[tracing::instrument(level = "debug", name = "on_stream_received", skip_all)]
    async fn on_stream_received(connection: &Connection, stream: BoxedReceiveStream) -> bool {
        let incoming = match connection.control.version().accept_uni_stream(stream).await {
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
                    connection,
                    receiver.track_alias,
                    IncomingObject::Subgroup(receiver),
                )
                .await;
            }
            IncomingStream::Fetch(receiver) => {
                FetchNotifier::notify(connection, receiver.request_id, receiver).await;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::{
        modules::test_support::{
            HANDSHAKE_TIMEOUT, open_subgroup, published_track, send_payload,
            subscribed_track_reader,
        },
        wire::SubgroupId,
    };

    #[tokio::test]
    async fn a_stream_without_its_header_does_not_hold_back_later_streams() {
        // Arrange
        let track = published_track("uni-header-pending").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let _headerless = factory.next().await.unwrap();
        let mut group_1 = open_subgroup(&factory, 1, SubgroupId::None).await;

        // Act
        send_payload(&mut group_1, b"g1-o0").await;
        let object = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            let mut reader = subscribed_track_reader(&track.server, &track.accepted).await;
            reader.next_object().await
        })
        .await;

        // Assert
        let object = object
            .expect("the later stream is read within the timeout")
            .unwrap()
            .expect("track still open");
        assert_eq!(object.payload, Bytes::from_static(b"g1-o0"));
    }
}
