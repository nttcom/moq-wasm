use crate::modules::moqt::control_plane::error_codes::DataStreamResetCode;
use crate::modules::transport::transport_connection::BoxedSendStream;
use std::marker::PhantomData;

use crate::modules::moqt::{
    data_plane::{
        object::canonical_object::{Object, SubgroupStart},
        stream::stream_sender::StreamSender,
        stream_priority::StreamPriority,
    },
    domains::send_targets::any_sent,
    version::{SubgroupEncoder, Version},
};

/// Typestate: header not yet sent.
pub struct Uninitialized;
/// Typestate: header has been sent.
pub struct HeaderSent;

pub type SubgroupHeaderSender = StreamDataSender<Uninitialized>;
pub type SubgroupObjectSender = StreamDataSender<HeaderSent>;

/// Handles sending data on a subgroup stream.
///
/// In the `S = Uninitialized` state the header can be sent; `start` consumes
/// the sender and returns it in the `S = HeaderSent` state. Objects can only
/// be sent in the `HeaderSent` state, so the invariant that the header is
/// always sent first is enforced at compile time.
///
/// While a session migrates, one sender writes the same subgroup to a stream
/// on each connection that takes the track (draft-22 §7.6.1). A stream that
/// fails is dropped as long as another one is left; the failure is returned
/// when it was the last.
pub struct StreamDataSender<S = Uninitialized> {
    streams: Vec<WireStream>,
    _state: PhantomData<S>,
}

/// One stream of the subgroup, on one connection, with that connection's
/// track alias.
pub(crate) struct WireStream {
    stream_sender: StreamSender,
    encoder: SubgroupEncoder,
}

impl WireStream {
    pub(crate) fn new(version: Version, track_alias: u64, send_stream: BoxedSendStream) -> Self {
        Self {
            stream_sender: StreamSender::new(send_stream),
            encoder: SubgroupEncoder::new(version, track_alias),
        }
    }
}

impl<S> StreamDataSender<S> {
    /// Drops the streams whose step failed; when every step failed, keeps
    /// them all and returns the first error.
    fn settle(&mut self, results: Vec<anyhow::Result<()>>) -> anyhow::Result<()> {
        if results.iter().all(Result::is_err) {
            return any_sent(results);
        }
        let mut results = results.into_iter();
        self.streams.retain(|_| match results.next() {
            Some(Err(error)) => {
                tracing::debug!(%error, "dropping one stream of a subgroup sent on several connections");
                false
            }
            _ => true,
        });
        Ok(())
    }

    pub async fn close(&mut self) -> anyhow::Result<()> {
        let mut results = Vec::with_capacity(self.streams.len());
        for stream in &self.streams {
            results.push(stream.stream_sender.close().await);
        }
        self.settle(results)
    }

    pub async fn reset(&mut self, code: DataStreamResetCode) -> anyhow::Result<()> {
        let mut results = Vec::with_capacity(self.streams.len());
        for stream in &self.streams {
            let code = stream.encoder.version().data_stream_reset_code(code);
            results.push(stream.stream_sender.reset(code).await);
        }
        self.settle(results)
    }
}

// ─── Uninitialized State ───────────────────────────────────────────────────────

impl StreamDataSender<Uninitialized> {
    /// `streams` must not be empty.
    pub(crate) fn on_streams(streams: Vec<WireStream>) -> Self {
        Self {
            streams,
            _state: PhantomData,
        }
    }

    /// Fails only when no stream took the priority; a stream that did not
    /// fails at its next send.
    pub async fn set_priority(&self, priority: StreamPriority) -> anyhow::Result<()> {
        let priority = priority.transport_priority();
        let mut results = Vec::with_capacity(self.streams.len());
        for stream in &self.streams {
            results.push(stream.stream_sender.set_priority(priority).await);
        }
        any_sent(results)
    }

    /// Sends the header of the subgroup `start` describes and returns the
    /// sender for its objects.
    pub async fn start(
        mut self,
        start: SubgroupStart,
    ) -> anyhow::Result<StreamDataSender<HeaderSent>> {
        tracing::debug!(?start, "Sending new subgroup header");
        let mut results = Vec::with_capacity(self.streams.len());
        for stream in &mut self.streams {
            let header = stream.encoder.header(start);
            results.push(stream.stream_sender.send(&header).await);
        }
        self.settle(results)?;
        Ok(self.into_header_sent())
    }

    /// Sends a draft-14 header without extensions.
    #[cfg(test)]
    pub(crate) async fn start_raw(
        mut self,
        group_id: u64,
        subgroup_id: crate::wire::SubgroupId,
        publisher_priority: u8,
    ) -> anyhow::Result<StreamDataSender<HeaderSent>> {
        let mut results = Vec::with_capacity(self.streams.len());
        for stream in &mut self.streams {
            let SubgroupEncoder::Draft14(encoder) = &mut stream.encoder;
            let bytes = encoder.raw_header(group_id, subgroup_id.clone(), publisher_priority);
            results.push(stream.stream_sender.send(&bytes).await);
        }
        self.settle(results)?;
        Ok(self.into_header_sent())
    }

    fn into_header_sent(self) -> StreamDataSender<HeaderSent> {
        StreamDataSender {
            streams: self.streams,
            _state: PhantomData,
        }
    }
}

impl StreamDataSender<HeaderSent> {
    pub async fn send_object(&mut self, object: &Object) -> anyhow::Result<()> {
        let mut results = Vec::with_capacity(self.streams.len());
        for stream in &mut self.streams {
            let result = match stream.encoder.object(object) {
                Ok(bytes) => stream.stream_sender.send(&bytes).await,
                Err(error) => Err(error),
            };
            results.push(result);
        }
        self.settle(results)
    }

    #[cfg(test)]
    pub(crate) async fn send_raw(
        &mut self,
        object_id_delta: u64,
        subgroup_object: crate::wire::SubgroupObject,
    ) -> anyhow::Result<()> {
        let mut results = Vec::with_capacity(self.streams.len());
        for stream in &mut self.streams {
            let SubgroupEncoder::Draft14(encoder) = &stream.encoder;
            let bytes = encoder.raw_object_field(object_id_delta, subgroup_object.clone());
            results.push(stream.stream_sender.send(&bytes).await);
        }
        self.settle(results)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::{
        modules::test_support::{
            HANDSHAKE_TIMEOUT, first_subgroup_stream, object_with_extensions, published_track,
        },
        object::{
            DataStreamResetCode, ForwardingPreference, Location, Object, ObjectStatus, ReadError,
            StreamReceiveError, Subgroup, SubgroupStart,
        },
    };

    const START: SubgroupStart = SubgroupStart {
        group_id: 3,
        subgroup_id: 1,
        publisher_priority: 9,
        ends_group: true,
    };

    fn object(object_id: u64, status: ObjectStatus, payload: &'static [u8]) -> Object {
        Object {
            location: Location {
                group_id: START.group_id,
                object_id,
            },
            forwarding: ForwardingPreference::Subgroup {
                subgroup_id: START.subgroup_id,
            },
            publisher_priority: START.publisher_priority,
            status,
            payload: Bytes::from_static(payload),
            ..object_with_extensions()
        }
    }

    #[tokio::test]
    async fn the_peer_receives_the_subgroup_and_objects_as_sent() {
        // Arrange
        let track = published_track("sender-round-trip").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut sender = factory.next().await.unwrap().start(START).await.unwrap();
        let sent = [
            object(63, ObjectStatus::Normal, b"\x00a\xff"),
            object(64, ObjectStatus::Normal, b""),
            object(16_384, ObjectStatus::Normal, b"\x80b\x00"),
            object(16_385, ObjectStatus::EndOfGroup, b""),
        ];

        tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            // Act
            for object in &sent {
                sender.send_object(object).await.unwrap();
            }
            sender.close().await.unwrap();

            // Assert
            let mut receiver = first_subgroup_stream(&track.server, &track.accepted).await;
            let header = receiver.receive().await.unwrap();
            assert!(matches!(header, Some(Subgroup::Header(start)) if start == START));
            for expected in sent {
                let Some(Subgroup::Object(received)) = receiver.receive().await.unwrap() else {
                    panic!("expected an object before FIN");
                };
                assert_eq!(received, expected);
            }
            assert!(receiver.receive().await.unwrap().is_none());
        })
        .await
        .expect("subgroup objects and FIN arrive within timeout");
    }

    #[tokio::test]
    async fn an_empty_subgroup_returns_its_header_then_fin() {
        for subgroup_id in [0, START.subgroup_id] {
            // Arrange
            let track = published_track("sender-empty-fin").await;
            let factory = track.client.publisher().create_stream(&track.published);
            let start = SubgroupStart {
                subgroup_id,
                ends_group: false,
                ..START
            };
            let mut sender = factory.next().await.unwrap().start(start).await.unwrap();

            tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
                // Act
                sender.close().await.unwrap();
                let mut receiver = first_subgroup_stream(&track.server, &track.accepted).await;
                let header = receiver.receive().await.unwrap();
                let end = receiver.receive().await.unwrap();

                // Assert
                assert!(matches!(header, Some(Subgroup::Header(received)) if received == start));
                assert!(end.is_none(), "{end:?}");
            })
            .await
            .expect("empty subgroup finishes within timeout");
        }
    }

    #[tokio::test]
    async fn a_reset_after_an_object_is_not_a_clean_fin() {
        // Arrange
        let track = published_track("sender-reset").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut sender = factory.next().await.unwrap().start(START).await.unwrap();
        let sent = object(63, ObjectStatus::Normal, b"\x00before-reset\xff");
        let mut receiver = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            sender.send_object(&sent).await.unwrap();
            let mut receiver = first_subgroup_stream(&track.server, &track.accepted).await;
            let Some(Subgroup::Header(_)) = receiver.receive().await.unwrap() else {
                panic!("expected a subgroup header before RESET");
            };
            let Some(Subgroup::Object(_)) = receiver.receive().await.unwrap() else {
                panic!("expected an object before RESET");
            };
            receiver
        })
        .await
        .expect("subgroup object arrives before reset within timeout");

        tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            // Act
            sender
                .reset(DataStreamResetCode::InternalError)
                .await
                .unwrap();
            let result = receiver.receive().await;

            // Assert
            assert!(
                matches!(result, Err(StreamReceiveError::Closed(ReadError::Reset))),
                "{result:?}"
            );
        })
        .await
        .expect("subgroup reset reaches the receiver within timeout");
    }

    #[tokio::test]
    async fn an_object_id_that_does_not_increase_is_rejected() {
        // Arrange
        let track = published_track("sender-decreasing").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut sender = factory.next().await.unwrap().start(START).await.unwrap();
        sender
            .send_object(&object(2, ObjectStatus::Normal, b"a"))
            .await
            .unwrap();

        // Act
        let result = sender
            .send_object(&object(2, ObjectStatus::Normal, b"b"))
            .await;

        // Assert
        assert!(result.is_err());
    }
}
