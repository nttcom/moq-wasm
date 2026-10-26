use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

use crate::modules::executor;
use crate::modules::moqt::control_plane::error_codes::DataStreamResetCode;
use crate::modules::moqt::{
    control_plane::control_messages::messages::parameters::location::Location,
    data_plane::{object::canonical_object::Object, stream::stream_sender::StreamSender},
    domains::connection::Connection,
    version::{FetchEncoder, Version},
};
use crate::modules::transport::{
    transport_connection::BoxedSendStream, transport_send_stream::TransportSendError,
};
use futures::FutureExt;

/// What `FetchDataSender::report_unknown_range` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnknownRange {
    /// The response marks the range unknown and goes on.
    Marked,
    /// The response was reset with UNKNOWN_OBJECT_STATUS and has ended.
    Reset,
}

/// Dropped without `close` or `reset`, the response is incomplete, so it is
/// reset with INTERNAL_ERROR.
pub struct FetchDataSender {
    version: Version,
    /// Shared with the request registry, which resets it when the FETCH is
    /// cancelled (draft-18 §5.2).
    stream_sender: Arc<StreamSender>,
    /// Held across the send, so objects reach the stream in the order they
    /// were encoded.
    encoder: tokio::sync::Mutex<FetchEncoder>,
    connection: Weak<Connection>,
    /// The FETCH's Request ID on `connection`.
    request_id: u64,
    /// Set by `close` and `reset`.
    ended: AtomicBool,
}

impl FetchDataSender {
    /// Fails when the FETCH `request_id` is no longer open, having reset the
    /// stream: a cancelled FETCH gets no response (draft-18 §5.2).
    // On wasm32 the browser transport is !Send, but the sender is shared with
    // the native build, where the tasks run on a multi-threaded executor.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub(crate) async fn new(
        connection: &Arc<Connection>,
        send_stream: BoxedSendStream,
        request_id: u64,
    ) -> anyhow::Result<Self> {
        let version = connection.control.version();
        let stream_sender = Arc::new(StreamSender::new(send_stream));
        if !connection
            .control
            .attach_fetch_response(request_id, &stream_sender)
        {
            stream_sender
                .reset(version.data_stream_reset_code(DataStreamResetCode::Cancelled))
                .await?;
            anyhow::bail!("FETCH {request_id} is no longer open");
        }
        stream_sender
            .send(&version.encode_fetch_header(request_id))
            .await?;
        Ok(Self {
            version,
            stream_sender,
            encoder: tokio::sync::Mutex::new(FetchEncoder::new(version)),
            connection: Arc::downgrade(connection),
            request_id,
            ended: AtomicBool::new(false),
        })
    }

    /// Sends `object` on this FETCH response. draft-14 sends a datagram
    /// object with its Object ID as the Subgroup ID (§10.4.4) and a
    /// zero-length object as its status; draft-18 carries only Normal
    /// objects and skips the others.
    pub async fn send_object(&self, object: &Object) -> anyhow::Result<()> {
        let mut encoder = self.encoder.lock().await;
        let Some(bytes) = encoder.object(object)? else {
            return Ok(());
        };
        let sent = self.stream_sender.send(&bytes).await;
        drop(encoder);
        if let Err(TransportSendError::Stopped { .. }) = &sent {
            self.stopped_by_peer().await;
        }
        Ok(sent?)
    }

    /// Reports that the objects after the last one sent (or the start of the
    /// range) through `last` have unknown status, so they may exist
    /// (draft-18 §11.4.4.2 End of Unknown Range); objects after `last` can
    /// still be sent. Where the session's draft cannot say so (draft-14, or
    /// a draft-18 range ending in the group of the last object sent), the
    /// response is reset with UNKNOWN_OBJECT_STATUS instead (draft-18
    /// §10.12) and has ended.
    pub async fn report_unknown_range(&self, last: Location) -> anyhow::Result<UnknownRange> {
        let marker = self.encoder.lock().await.end_of_unknown_range(last);
        match marker {
            Some(bytes) => {
                self.stream_sender.send(&bytes).await?;
                Ok(UnknownRange::Marked)
            }
            None => {
                self.reset(DataStreamResetCode::UnknownObjectStatus).await?;
                Ok(UnknownRange::Reset)
            }
        }
    }

    /// Ends the response with FIN: every object of the range this side
    /// knows was sent. Fails when the peer has stopped the stream, which
    /// cancels the FETCH instead.
    pub async fn close(&self) -> anyhow::Result<()> {
        self.ended.store(true, Ordering::Relaxed);
        if let Some(Some(code)) = self.stream_sender.stopped().now_or_never() {
            self.stopped_by_peer().await;
            return Err(TransportSendError::Stopped { code }.into());
        }
        self.stream_sender.close().await?;
        response_ended(&self.connection, self.request_id).await
    }

    async fn stopped_by_peer(&self) {
        if let Some(connection) = self.connection.upgrade() {
            connection
                .control
                .fetch_response_stopped(&connection, self.request_id)
                .await;
        }
    }

    pub async fn reset(&self, code: DataStreamResetCode) -> anyhow::Result<()> {
        self.ended.store(true, Ordering::Relaxed);
        self.stream_sender
            .reset(self.version.data_stream_reset_code(code))
            .await?;
        response_ended(&self.connection, self.request_id).await
    }
}

impl Drop for FetchDataSender {
    fn drop(&mut self) {
        if self.ended.load(Ordering::Relaxed) {
            return;
        }
        let stream_sender = self.stream_sender.clone();
        let code = self
            .version
            .data_stream_reset_code(DataStreamResetCode::InternalError);
        let connection = self.connection.clone();
        let request_id = self.request_id;
        executor::try_spawn("reset an unfinished FETCH response", async move {
            if let Err(error) = stream_sender.reset(code).await {
                tracing::debug!(%error, "could not reset an unfinished FETCH response");
            }
            if let Err(error) = response_ended(&connection, request_id).await {
                tracing::debug!(%error, "could not end the request of a FETCH response");
            }
        });
    }
}

async fn response_ended(connection: &Weak<Connection>, request_id: u64) -> anyhow::Result<()> {
    if let Some(connection) = connection.upgrade() {
        connection.control.fetch_response_ended(request_id).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;

    use super::{FetchDataSender, UnknownRange};
    use crate::{
        Session, SessionEvent,
        fetch::FetchOption,
        modules::{
            moqt::{
                control_plane::control_messages::messages::parameters::location::Location,
                data_plane::object::{
                    canonical_object::{ForwardingPreference, Object},
                    object_status::ObjectStatus,
                },
                version::Version,
            },
            test_support::{spawn_connected_draft18_sessions, spawn_connected_dual_sessions},
            transport::transport_send_stream::MockTransportSendStream,
        },
    };

    /// A send stream that accepts every write and reports the code it is
    /// reset with.
    fn recording_stream() -> (
        MockTransportSendStream,
        tokio::sync::mpsc::UnboundedReceiver<u64>,
    ) {
        let (reset_sender, reset_receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut stream = MockTransportSendStream::new();
        stream.expect_send().returning(|_| Ok(()));
        stream.expect_reset().returning(move |code| {
            let _ = reset_sender.send(code);
            Ok(())
        });
        (stream, reset_receiver)
    }

    /// A FETCH from `client` that `server` holds open, answered on
    /// `stream`.
    async fn answering_fetch(
        client: &Session,
        server: &Session,
        stream: MockTransportSendStream,
    ) -> FetchDataSender {
        let mut subscriber = client.subscriber();
        tokio::spawn(async move {
            let _ = subscriber
                .fetch(
                    "ns".to_string(),
                    "track".to_string(),
                    Location {
                        group_id: 0,
                        object_id: 0,
                    },
                    Location {
                        group_id: 2,
                        object_id: 0,
                    },
                    FetchOption::default(),
                )
                .await;
        });
        let SessionEvent::Fetch(handler) = server.receive_event().await.unwrap() else {
            panic!("expected FETCH");
        };
        let (connection, wire_request_id) = server
            .publisher()
            .session
            .connection_of(handler.request_id)
            .unwrap();
        FetchDataSender::new(&connection, Box::new(stream), wire_request_id)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_fetch_sender_dropped_unended_resets_with_internal_error() {
        for (version, (client, server)) in [
            (
                Version::Draft14,
                spawn_connected_dual_sessions("fetch-sender-dropped-14").await,
            ),
            (
                Version::Draft18,
                spawn_connected_draft18_sessions("fetch-sender-dropped-18", "moqt").await,
            ),
        ] {
            // Arrange
            assert_eq!(
                server.publisher().session.connection().control.version(),
                version
            );
            let (stream, mut reset_receiver) = recording_stream();
            let sender = answering_fetch(&client, &server, stream).await;

            // Act
            drop(sender);
            let reset = tokio::time::timeout(Duration::from_secs(3), reset_receiver.recv()).await;

            // Assert: INTERNAL_ERROR (0x0) in draft-14 §10.4.3 and draft-18 §3.3.3
            assert_eq!(reset, Ok(Some(0x0)), "{version:?}");
        }
    }

    #[tokio::test]
    async fn an_unknown_range_the_draft_cannot_express_resets_with_unknown_object_status() {
        // Arrange: draft-14 has no End of Unknown Range; on draft-18 the range
        // ends in the group of the object sent before it
        for (version, (client, server), wire_code) in [
            (
                Version::Draft14,
                spawn_connected_dual_sessions("fetch-sender-unknown-14").await,
                0x0,
            ),
            (
                Version::Draft18,
                spawn_connected_draft18_sessions("fetch-sender-unknown-18", "moqt").await,
                0x6,
            ),
        ] {
            let (stream, mut reset_receiver) = recording_stream();
            let sender = answering_fetch(&client, &server, stream).await;
            sender
                .send_object(&Object {
                    location: Location {
                        group_id: 1,
                        object_id: 0,
                    },
                    forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
                    publisher_priority: 128,
                    status: ObjectStatus::Normal,
                    extension_headers: Default::default(),
                    payload: Bytes::from_static(b"o"),
                    ends_group: false,
                })
                .await
                .unwrap();

            // Act
            let reported = sender
                .report_unknown_range(Location {
                    group_id: 1,
                    object_id: 4,
                })
                .await;

            // Assert: draft-18 §10.12, §3.3.3 UNKNOWN_OBJECT_STATUS (0x6);
            // draft-14 §10.4.3 defines no such code and gets INTERNAL_ERROR (0x0)
            assert_eq!(reported.unwrap(), UnknownRange::Reset, "{version:?}");
            assert_eq!(reset_receiver.try_recv(), Ok(wire_code), "{version:?}");
            drop(sender);
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(
                reset_receiver.try_recv().is_err(),
                "reset once: {version:?}"
            );
        }
    }
}
