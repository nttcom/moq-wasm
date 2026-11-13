use std::sync::Arc;

use crate::modules::moqt::{
    data_plane::stream::fetch_data_receiver::FetchDataReceiver, domains::connection::Connection,
    version::ControlChannel,
};

pub(crate) struct FetchNotifier;

impl FetchNotifier {
    #[tracing::instrument(
        level = "debug",
        name = "moqt.fetch_notifier.notify",
        skip_all,
        fields(request_id = request_id)
    )]
    pub(crate) async fn notify(
        connection: &Arc<Connection>,
        request_id: u64,
        mut fetch_data_receiver: FetchDataReceiver,
    ) {
        // Draft-14 §9.16.3: a FETCH response is delivered on a single stream.
        let sender = connection
            .fetch_notification_map
            .lock()
            .expect("fetch_notification_map poisoned")
            .remove(&request_id);
        if let Some(sender) = sender {
            // Only the stream handed to this side's FETCH may end its request.
            fetch_data_receiver
                .end_request_with(ControlChannel::fetch_request_end(connection, request_id));
            if let Err(e) = sender.send(fetch_data_receiver) {
                tracing::warn!(error = %e, "failed to notify a fetch stream");
            }
        } else {
            // A FETCH this side cancelled can cross its stream, which the
            // publisher closes only once it reads FETCH_CANCEL (§9.19).
            if connection.control.wrote_request_id(request_id) {
                tracing::debug!(request_id, "stopping the stream of a FETCH no longer open");
            } else {
                tracing::error!(request_id, "no fetch stream sender for the request");
            }
            fetch_data_receiver.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        Session, SessionEvent, Version,
        fetch::{FetchHandle, FetchHandler, FetchOption},
        modules::{
            moqt::control_plane::error_codes::DataStreamResetCode,
            test_support::{
                DRAFTS, capture_telemetry, connected_sessions, location, send_until_stopped, within,
            },
            transport::transport_send_stream::TransportSendError,
        },
    };

    const NO_SENDER: &str = "no fetch stream sender for the request";
    const NO_LONGER_OPEN: &str = "stopping the stream of a FETCH no longer open";

    /// Dropping a quinn stream stops it with 0, so CANCELLED tells an
    /// explicit stop.
    fn stop_code(error: &anyhow::Error) -> Option<u64> {
        match TransportSendError::in_chain(error) {
            Some(TransportSendError::Stopped { code }) => Some(*code),
            _ => None,
        }
    }

    fn cancelled() -> Option<u64> {
        Some(Version::Draft14.data_stream_reset_code(DataStreamResetCode::Cancelled))
    }

    async fn answered_fetch(client: &Session, server: &Session) -> (FetchHandle, FetchHandler) {
        let mut subscriber = client.subscriber();
        let (handle, handler) = within(async {
            tokio::join!(
                subscriber.fetch(
                    "ns".into(),
                    "track".into(),
                    location(0, 0),
                    location(1, 0),
                    FetchOption::default(),
                ),
                async {
                    let SessionEvent::Fetch(handler) = server.receive_event().await.unwrap() else {
                        panic!("expected FETCH from the client");
                    };
                    handler.ok(false, location(1, 0)).await.unwrap();
                    handler
                }
            )
        })
        .await;
        (handle.unwrap(), handler)
    }

    #[tokio::test]
    async fn the_stream_of_a_cancelled_fetch_is_stopped_without_an_error() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();
        let (client, server) =
            connected_sessions("fetch-notifier-cancelled", Version::Draft14).await;
        let (handle, handler) = answered_fetch(&client, &server).await;
        client.subscriber().fetch_cancel(&handle).await.unwrap();
        let SessionEvent::FetchCancel(_) = within(server.receive_event()).await.unwrap() else {
            panic!("expected FETCH_CANCEL from the client");
        };

        // Act
        let sender = server
            .publisher()
            .create_fetch_stream(handler.request_id)
            .await
            .unwrap();
        let stopped = within(send_until_stopped(&sender)).await;

        // Assert
        assert!(telemetry.events(NO_SENDER).is_empty());
        let request_id = handler.request_id.to_string();
        let no_longer_open = telemetry.events(NO_LONGER_OPEN);
        assert_eq!(no_longer_open.len(), 1);
        assert_eq!(no_longer_open[0].field("request_id"), Some(&*request_id));
        assert_eq!(stop_code(&stopped), cancelled(), "{stopped:#}");
    }

    #[tokio::test]
    async fn the_stream_of_a_fetch_never_sent_is_stopped_with_an_error() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();
        let (_client, server) =
            connected_sessions("fetch-notifier-never-sent", Version::Draft14).await;
        // The client's parity, far above any Request ID it has used.
        let never_sent = 1_000;

        // Act
        let sender = server
            .publisher()
            .create_fetch_stream(never_sent)
            .await
            .unwrap();
        let stopped = within(send_until_stopped(&sender)).await;

        // Assert
        let no_sender = telemetry.events(NO_SENDER);
        assert_eq!(no_sender.len(), 1);
        assert_eq!(no_sender[0].field("request_id"), Some("1000"));
        assert_eq!(stop_code(&stopped), cancelled(), "{stopped:#}");
    }

    #[tokio::test]
    async fn only_a_request_id_this_side_wrote_counts_as_written() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("fetch-notifier-written", version).await;
            let (_handle, handler) = answered_fetch(&client, &server).await;
            let control = &client.subscriber().session.connection().control;
            let written = handler.request_id;

            // Act / Assert
            assert!(control.wrote_request_id(written));
            assert!(!control.wrote_request_id(written + 1));
            assert!(!control.wrote_request_id(written + 2));
        }
    }
}
