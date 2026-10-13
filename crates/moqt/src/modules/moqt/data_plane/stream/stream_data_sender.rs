use crate::modules::moqt::control_plane::error_codes::DataStreamResetCode;
use crate::modules::transport::transport_connection::BoxedSendStream;
use std::marker::PhantomData;

use crate::modules::moqt::data_plane::{
    object::{
        canonical_object::{Object, SubgroupStart},
        extension_headers::ExtensionHeaders,
        subgroup::{SubgroupHeader, SubgroupId, SubgroupObject, SubgroupObjectField},
    },
    stream::stream_sender::StreamSender,
    stream_priority::StreamPriority,
};

/// Typestate: header not yet sent.
pub struct Uninitialized;
/// Typestate: header has been sent.
pub struct HeaderSent;

pub type SubgroupHeaderSender = StreamDataSender<Uninitialized>;
pub type SubgroupObjectSender = StreamDataSender<HeaderSent>;

/// Handles sending data on a subgroup stream.
///
/// In the `S = Uninitialized` state a header can be created and sent.
/// Calling `send_header` consumes the sender and transitions it to `S = HeaderSent`.
/// Objects can only be sent in the `HeaderSent` state, so the invariant that
/// the header is always sent first is enforced at compile time.
pub struct StreamDataSender<S = Uninitialized> {
    stream_sender: StreamSender,
    track_alias: u64,
    subgroup_header: Option<SubgroupHeader>,
    prev_object_id: Option<u64>,
    _state: PhantomData<S>,
}

// ─── Uninitialized State ───────────────────────────────────────────────────────

impl StreamDataSender<Uninitialized> {
    pub(crate) fn new(track_alias: u64, send_stream: BoxedSendStream) -> Self {
        let stream_sender = StreamSender::new(send_stream);
        Self {
            stream_sender,
            track_alias,
            subgroup_header: None,
            prev_object_id: None,
            _state: PhantomData,
        }
    }

    /// Creates a subgroup header associated with this stream.
    pub(crate) fn create_header(
        &self,
        group_id: u64,
        subgroup_id: SubgroupId,
        publisher_priority: u8,
        end_of_group: bool,
        has_extensions: bool,
    ) -> SubgroupHeader {
        SubgroupHeader::new(
            self.track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
            has_extensions,
            end_of_group,
        )
    }

    pub async fn set_priority(&self, priority: StreamPriority) -> anyhow::Result<()> {
        self.stream_sender
            .set_priority(priority.transport_priority())
            .await
    }

    /// Sends the header of the subgroup `start` describes and returns the
    /// sender for its objects. Extensions are declared present, so every
    /// object may carry them.
    pub async fn start(self, start: SubgroupStart) -> anyhow::Result<StreamDataSender<HeaderSent>> {
        let header = SubgroupHeader::new(
            self.track_alias,
            start.group_id,
            SubgroupId::Value(start.subgroup_id),
            start.publisher_priority,
            true,
            start.ends_group,
        );
        self.send_header(header).await
    }

    pub(crate) async fn send_header(
        self,
        header: SubgroupHeader,
    ) -> anyhow::Result<StreamDataSender<HeaderSent>> {
        if header.track_alias != self.track_alias {
            anyhow::bail!(
                "track_alias mismatch: expected {}, got {}",
                self.track_alias,
                header.track_alias
            );
        }
        tracing::debug!("Sending new subgroup header: {:?}", header);
        let encoded_header = header.encode();
        self.stream_sender.send(&encoded_header).await?;
        Ok(StreamDataSender {
            stream_sender: self.stream_sender,
            track_alias: self.track_alias,
            subgroup_header: Some(header),
            prev_object_id: None,
            _state: PhantomData,
        })
    }

    pub async fn close(&mut self) -> anyhow::Result<()> {
        self.stream_sender.close().await
    }

    pub async fn reset(&mut self, code: DataStreamResetCode) -> anyhow::Result<()> {
        self.stream_sender.reset(code.into()).await
    }
}

// ─── HeaderSent State ──────────────────────────────────────────────────────────

impl StreamDataSender<HeaderSent> {
    /// Sends `object` on this subgroup with the header's group, subgroup and
    /// priority. Fails when its object id is not greater than the previous
    /// object's. An object without payload is sent as its status.
    pub async fn send_object(&mut self, object: &Object) -> anyhow::Result<()> {
        let object_id = object.location.object_id;
        let object_id_delta = match self.prev_object_id {
            None => Some(object_id),
            Some(prev) => prev
                .checked_add(1)
                .and_then(|next| object_id.checked_sub(next)),
        }
        .ok_or_else(|| {
            anyhow::anyhow!(
                "object {object_id} does not follow object {:?} in its subgroup",
                self.prev_object_id
            )
        })?;
        let subgroup_object = if object.payload.is_empty() {
            SubgroupObject::new_status(u8::from(object.status) as u64)
        } else {
            SubgroupObject::new_payload(object.payload.clone())
        };
        let field = self.create_object_field(
            object_id_delta,
            object.extension_headers.clone(),
            subgroup_object,
        );
        self.send(field).await?;
        self.prev_object_id = Some(object_id);
        Ok(())
    }

    pub(crate) fn create_object_field(
        &self,
        object_id_delta: u64,
        extension_headers: ExtensionHeaders,
        subgroup_object: SubgroupObject,
    ) -> SubgroupObjectField {
        let header = self.subgroup_header.as_ref().unwrap();
        SubgroupObjectField {
            message_type: header.message_type,
            object_id_delta,
            extension_headers,
            subgroup_object,
        }
    }

    /// Sends an object on the stream.
    pub(crate) async fn send(&mut self, data: SubgroupObjectField) -> anyhow::Result<()> {
        let bytes = data.encode();
        self.stream_sender.send(&bytes).await
    }

    pub async fn close(&mut self) -> anyhow::Result<()> {
        self.stream_sender.close().await
    }

    pub async fn reset(&mut self, code: DataStreamResetCode) -> anyhow::Result<()> {
        self.stream_sender.reset(code.into()).await
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
