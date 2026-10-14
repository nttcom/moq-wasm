use crate::modules::moqt::control_plane::error_codes::DataStreamResetCode;
use crate::modules::transport::transport_connection::BoxedSendStream;
use std::marker::PhantomData;

use crate::modules::moqt::{
    data_plane::{
        object::canonical_object::{Object, SubgroupStart},
        stream::stream_sender::StreamSender,
        stream_priority::StreamPriority,
    },
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
pub struct StreamDataSender<S = Uninitialized> {
    stream_sender: StreamSender,
    encoder: SubgroupEncoder,
    _state: PhantomData<S>,
}

// ─── Uninitialized State ───────────────────────────────────────────────────────

impl StreamDataSender<Uninitialized> {
    pub(crate) fn new(version: Version, track_alias: u64, send_stream: BoxedSendStream) -> Self {
        Self {
            stream_sender: StreamSender::new(send_stream),
            encoder: SubgroupEncoder::new(version, track_alias),
            _state: PhantomData,
        }
    }

    pub async fn set_priority(&self, priority: StreamPriority) -> anyhow::Result<()> {
        self.stream_sender
            .set_priority(priority.transport_priority())
            .await
    }

    /// Sends the header of the subgroup `start` describes and returns the
    /// sender for its objects.
    pub async fn start(
        mut self,
        start: SubgroupStart,
    ) -> anyhow::Result<StreamDataSender<HeaderSent>> {
        tracing::debug!(?start, "Sending new subgroup header");
        let header = self.encoder.header(start);
        self.stream_sender.send(&header).await?;
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
        let SubgroupEncoder::Draft14(encoder) = &mut self.encoder else {
            panic!("raw draft-14 fields need a draft-14 session");
        };
        let bytes = encoder.raw_header(group_id, subgroup_id, publisher_priority);
        self.stream_sender.send(&bytes).await?;
        Ok(self.into_header_sent())
    }

    fn into_header_sent(self) -> StreamDataSender<HeaderSent> {
        StreamDataSender {
            stream_sender: self.stream_sender,
            encoder: self.encoder,
            _state: PhantomData,
        }
    }

    pub async fn close(&mut self) -> anyhow::Result<()> {
        self.stream_sender.close().await
    }

    pub async fn reset(&mut self, code: DataStreamResetCode) -> anyhow::Result<()> {
        let code = self.encoder.version().data_stream_reset_code(code);
        self.stream_sender.reset(code).await
    }
}

// ─── HeaderSent State ──────────────────────────────────────────────────────────

impl StreamDataSender<HeaderSent> {
    /// Sends `object` on this subgroup with the header's group, subgroup and
    /// priority. Fails when its object id is not greater than the previous
    /// object's. An object without payload is sent as its status.
    pub async fn send_object(&mut self, object: &Object) -> anyhow::Result<()> {
        let bytes = self.encoder.object(object)?;
        self.stream_sender.send(&bytes).await
    }

    /// Sends a draft-14 object field without extensions.
    #[cfg(test)]
    pub(crate) async fn send_raw(
        &mut self,
        object_id_delta: u64,
        subgroup_object: crate::wire::SubgroupObject,
    ) -> anyhow::Result<()> {
        let SubgroupEncoder::Draft14(encoder) = &self.encoder else {
            panic!("raw draft-14 fields need a draft-14 session");
        };
        let bytes = encoder.raw_object_field(object_id_delta, subgroup_object);
        self.stream_sender.send(&bytes).await
    }

    pub async fn close(&mut self) -> anyhow::Result<()> {
        self.stream_sender.close().await
    }

    pub async fn reset(&mut self, code: DataStreamResetCode) -> anyhow::Result<()> {
        let code = self.encoder.version().data_stream_reset_code(code);
        self.stream_sender.reset(code).await
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::{
        modules::test_support::{HANDSHAKE_TIMEOUT, first_subgroup_stream, published_track},
        object::{
            ExtensionHeaders, ForwardingPreference, Location, Object, ObjectStatus, Subgroup,
            SubgroupStart,
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
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::from_static(payload),
            ends_group: false,
        }
    }

    #[tokio::test]
    async fn the_peer_receives_the_subgroup_and_objects_as_sent() {
        // Arrange
        let track = published_track("sender-round-trip").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut sender = factory.next().await.unwrap().start(START).await.unwrap();
        let sent = [
            object(0, ObjectStatus::Normal, b"a"),
            object(1, ObjectStatus::Normal, b""),
            object(4, ObjectStatus::Normal, b"b"),
            object(5, ObjectStatus::EndOfGroup, b""),
        ];

        // Act
        for object in &sent {
            sender.send_object(object).await.unwrap();
        }
        sender.close().await.unwrap();

        // Assert
        let mut receiver = first_subgroup_stream(&track.server, &track.accepted).await;
        let mut received = Vec::new();
        while let Some(item) = tokio::time::timeout(HANDSHAKE_TIMEOUT, receiver.receive())
            .await
            .unwrap()
            .unwrap()
        {
            received.push(item);
        }
        assert!(
            matches!(received.first(), Some(Subgroup::Header(start)) if *start == START),
            "{received:?}"
        );
        let objects: Vec<Object> = received
            .into_iter()
            .filter_map(|item| match item {
                Subgroup::Object(object) => Some(object),
                Subgroup::Header(_) => None,
            })
            .collect();
        assert_eq!(objects, sent);
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
