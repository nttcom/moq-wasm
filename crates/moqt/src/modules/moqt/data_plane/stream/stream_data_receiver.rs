use bytes::Bytes;

use crate::modules::moqt::{
    control_plane::control_messages::messages::parameters::location::Location,
    data_plane::{
        codec::{subgroup_decoder::SubgroupFrame, uni_stream_decoder::UniStreamData},
        object::{
            canonical_object::{ForwardingPreference, Object, SubgroupStart},
            object_status::ObjectStatus,
            subgroup::{SubgroupHeader, SubgroupId, SubgroupObject},
        },
        stream::stream_receiver::{StreamReceiveError, UniStreamReceiver},
    },
};

#[derive(Debug)]
pub enum Subgroup {
    Header(SubgroupStart),
    Object(Object),
}

#[derive(Debug)]
pub struct StreamDataReceiver {
    stream_receiver: UniStreamReceiver,
    pub track_alias: u64,
    header: SubgroupHeader,
    /// `None` until the first object arrives, for header types that take the
    /// subgroup id from the first object's id.
    subgroup_id: Option<u64>,
    header_returned: bool,
    prev_object_id: Option<u64>,
    first_object: Option<Object>,
}

impl StreamDataReceiver {
    pub(crate) fn new(stream: UniStreamReceiver, header: SubgroupHeader) -> Self {
        let subgroup_id = match header.subgroup_id {
            SubgroupId::None => Some(0),
            SubgroupId::Value(subgroup_id) => Some(subgroup_id),
            SubgroupId::FirstObjectIdDelta => None,
        };
        Self {
            stream_receiver: stream,
            track_alias: header.track_alias,
            header,
            subgroup_id,
            header_returned: false,
            prev_object_id: None,
            first_object: None,
        }
    }

    /// Returns the header first, then each object with absolute ids.
    /// `Ok(None)` means the peer finished the stream (FIN). A header whose
    /// subgroup id comes from the first object is returned only after that
    /// object arrives, so a stream that ends before it returns `Ok(None)`
    /// without a header.
    pub async fn receive(&mut self) -> Result<Option<Subgroup>, StreamReceiveError> {
        if !self.header_returned {
            if self.subgroup_id.is_none() {
                let Some(first) = self.next_object().await? else {
                    return Ok(None);
                };
                self.first_object = Some(first);
            }
            self.header_returned = true;
            return Ok(Some(Subgroup::Header(self.start())));
        }
        if let Some(first) = self.first_object.take() {
            return Ok(Some(Subgroup::Object(first)));
        }
        Ok(self.next_object().await?.map(Subgroup::Object))
    }

    fn start(&self) -> SubgroupStart {
        SubgroupStart {
            group_id: self.header.group_id,
            subgroup_id: self.subgroup_id.unwrap_or_default(),
            publisher_priority: self.header.publisher_priority,
            ends_group: self.header.message_type.has_end_of_group(),
        }
    }

    async fn next_object(&mut self) -> Result<Option<Object>, StreamReceiveError> {
        let field = match self.stream_receiver.receive().await? {
            Some(UniStreamData::Subgroup(SubgroupFrame::Object(field))) => field,
            // The decoder yields the header once, and the stream task took it.
            Some(UniStreamData::Subgroup(SubgroupFrame::Header(_))) => {
                unreachable!("a subgroup stream has one header")
            }
            Some(UniStreamData::Fetch(_)) => {
                unreachable!("Unexpected fetch data in subgroup stream")
            }
            None => {
                tracing::debug!("Stream data ended");
                return Ok(None);
            }
        };
        let object_id = field.resolve_object_id(self.prev_object_id);
        self.prev_object_id = Some(object_id);
        let subgroup_id = *self.subgroup_id.get_or_insert(object_id);
        let (status, payload) = match field.subgroup_object {
            SubgroupObject::Payload { data, .. } => (ObjectStatus::Normal, data),
            SubgroupObject::Status { code, .. } => {
                (status_from_code(code, object_id)?, Bytes::new())
            }
        };
        Ok(Some(Object {
            location: Location {
                group_id: self.header.group_id,
                object_id,
            },
            forwarding: ForwardingPreference::Subgroup { subgroup_id },
            publisher_priority: self.header.publisher_priority,
            status,
            extension_headers: field.extension_headers,
            payload,
            ends_group: false,
        }))
    }
}

/// draft-14 §10.2.1.1: an Object Status the draft does not define is a
/// protocol violation.
fn status_from_code(code: u64, object_id: u64) -> Result<ObjectStatus, StreamReceiveError> {
    u8::try_from(code)
        .ok()
        .and_then(|code| ObjectStatus::try_from(code).ok())
        .ok_or_else(|| {
            StreamReceiveError::ProtocolViolation(format!(
                "invalid object status {code:#x} on object {object_id}"
            ))
        })
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::Subgroup;
    use crate::{
        modules::test_support::{
            HANDSHAKE_TIMEOUT, first_subgroup_stream, open_subgroup, published_track,
        },
        object::{
            ExtensionHeaders, ForwardingPreference, Location, ObjectStatus, StreamReceiveError,
            SubgroupObjectSender, SubgroupStart,
        },
        wire::{SubgroupId, SubgroupObject},
    };

    async fn send_raw(sender: &mut SubgroupObjectSender, delta: u64, object: SubgroupObject) {
        let field = sender.create_object_field(delta, ExtensionHeaders::default(), object);
        sender.send(field).await.unwrap();
    }

    fn payload(bytes: &'static [u8]) -> SubgroupObject {
        SubgroupObject::new_payload(Bytes::from_static(bytes))
    }

    async fn receive(receiver: &mut super::StreamDataReceiver) -> Option<Subgroup> {
        tokio::time::timeout(HANDSHAKE_TIMEOUT, receiver.receive())
            .await
            .expect("receive within timeout")
            .unwrap()
    }

    #[tokio::test]
    async fn object_ids_are_resolved_from_their_deltas() {
        // Arrange: deltas 0, 0, 1 mean ids 0, 1, 3
        let track = published_track("receiver-deltas").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut sender = open_subgroup(&factory, 4, SubgroupId::Value(2)).await;
        send_raw(&mut sender, 0, payload(b"a")).await;
        send_raw(&mut sender, 0, payload(b"b")).await;
        send_raw(&mut sender, 1, payload(b"c")).await;
        let mut receiver = first_subgroup_stream(&track.server, &track.accepted).await;

        // Act
        let header = receive(&mut receiver).await;
        let mut ids = Vec::new();
        for _ in 0..3 {
            let Some(Subgroup::Object(object)) = receive(&mut receiver).await else {
                panic!("expected an object");
            };
            assert_eq!(
                object.forwarding,
                ForwardingPreference::Subgroup { subgroup_id: 2 }
            );
            ids.push(object.location);
        }

        // Assert
        assert!(
            matches!(
                header,
                Some(Subgroup::Header(SubgroupStart {
                    group_id: 4,
                    subgroup_id: 2,
                    ends_group: false,
                    ..
                }))
            ),
            "{header:?}"
        );
        assert_eq!(
            ids,
            [0, 1, 3].map(|object_id| Location {
                group_id: 4,
                object_id
            })
        );
    }

    #[tokio::test]
    async fn a_first_object_header_is_returned_with_the_first_object_id() {
        // Arrange
        let track = published_track("receiver-first-object").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut sender = open_subgroup(&factory, 0, SubgroupId::FirstObjectIdDelta).await;
        send_raw(&mut sender, 5, payload(b"a")).await;
        let mut receiver = first_subgroup_stream(&track.server, &track.accepted).await;

        // Act
        let header = receive(&mut receiver).await;
        let object = receive(&mut receiver).await;

        // Assert
        assert!(
            matches!(
                header,
                Some(Subgroup::Header(SubgroupStart { subgroup_id: 5, .. }))
            ),
            "{header:?}"
        );
        assert!(
            matches!(&object, Some(Subgroup::Object(object)) if object.location.object_id == 5),
            "{object:?}"
        );
    }

    #[tokio::test]
    async fn a_first_object_stream_that_ends_empty_returns_no_header() {
        // Arrange
        let track = published_track("receiver-first-object-empty").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut sender = open_subgroup(&factory, 0, SubgroupId::FirstObjectIdDelta).await;
        let receiver = first_subgroup_stream(&track.server, &track.accepted);
        sender.close().await.unwrap();
        let mut receiver = receiver.await;

        // Act / Assert
        assert!(receive(&mut receiver).await.is_none());
    }

    #[tokio::test]
    async fn a_status_object_carries_its_status_and_no_payload() {
        // Arrange
        let track = published_track("receiver-status").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut sender = open_subgroup(&factory, 0, SubgroupId::None).await;
        send_raw(
            &mut sender,
            0,
            SubgroupObject::new_status(ObjectStatus::EndOfGroup as u64),
        )
        .await;
        let mut receiver = first_subgroup_stream(&track.server, &track.accepted).await;
        receive(&mut receiver).await;

        // Act
        let object = receive(&mut receiver).await;

        // Assert
        assert!(
            matches!(&object, Some(Subgroup::Object(object))
                if object.status == ObjectStatus::EndOfGroup && object.payload.is_empty()),
            "{object:?}"
        );
    }

    #[tokio::test]
    async fn an_unknown_object_status_is_a_protocol_violation() {
        // Arrange: draft-14 §10.2.1.1 defines no status 0x2
        let track = published_track("receiver-unknown-status").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut sender = open_subgroup(&factory, 0, SubgroupId::None).await;
        send_raw(&mut sender, 0, SubgroupObject::new_status(0x2)).await;
        let mut receiver = first_subgroup_stream(&track.server, &track.accepted).await;
        receive(&mut receiver).await;

        // Act
        let result = tokio::time::timeout(HANDSHAKE_TIMEOUT, receiver.receive())
            .await
            .unwrap();

        // Assert
        assert!(
            matches!(result, Err(StreamReceiveError::ProtocolViolation(_))),
            "{result:?}"
        );
    }
}
