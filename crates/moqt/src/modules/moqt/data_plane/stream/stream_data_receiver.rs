use crate::modules::moqt::{
    data_plane::{
        object::canonical_object::{Object, SubgroupStart},
        stream::{
            open_subgroups::{OpenSubgroup, OpenSubgroups},
            stream_receiver::StreamReceiveError,
        },
    },
    version::SubgroupReader,
};

#[derive(Debug)]
pub enum Subgroup {
    Header(SubgroupStart),
    Object(Object),
}

#[derive(Debug)]
pub struct StreamDataReceiver {
    pub track_alias: u64,
    reader: SubgroupReader,
    header_returned: bool,
    first_object: Option<Object>,
    open: Option<OpenSubgroup>,
}

impl StreamDataReceiver {
    pub(crate) fn new(track_alias: u64, reader: SubgroupReader) -> Self {
        Self {
            track_alias,
            reader,
            header_returned: false,
            first_object: None,
            open: None,
        }
    }

    pub(crate) fn group_id(&self) -> u64 {
        self.reader.group_id()
    }

    /// Counts this stream in `open_subgroups` until it ends or is dropped.
    pub(crate) fn count_in(&mut self, open_subgroups: &OpenSubgroups) {
        self.open = Some(open_subgroups.open());
    }

    /// Returns the header first, then each object with absolute ids.
    /// `Ok(None)` means the peer finished the stream (FIN). A header whose
    /// subgroup id comes from the first object is returned only after that
    /// object arrives, so a stream that ends before it returns `Ok(None)`
    /// without a header.
    pub async fn receive(&mut self) -> Result<Option<Subgroup>, StreamReceiveError> {
        let received = self.receive_next().await;
        if !matches!(received, Ok(Some(_))) {
            self.open = None;
        }
        received
    }

    async fn receive_next(&mut self) -> Result<Option<Subgroup>, StreamReceiveError> {
        if !self.header_returned {
            if self.reader.start().is_none() {
                let Some(first) = self.reader.next_object().await? else {
                    return Ok(None);
                };
                self.first_object = Some(first);
            }
            self.header_returned = true;
            let start = self
                .reader
                .start()
                .expect("the first object has fixed the subgroup id");
            return Ok(Some(Subgroup::Header(start)));
        }
        if let Some(first) = self.first_object.take() {
            return Ok(Some(Subgroup::Object(first)));
        }
        Ok(self.reader.next_object().await?.map(Subgroup::Object))
    }
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
            ForwardingPreference, Location, ObjectStatus, StreamReceiveError, SubgroupObjectSender,
            SubgroupStart,
        },
        wire::{SubgroupId, SubgroupObject},
    };

    async fn send_raw(sender: &mut SubgroupObjectSender, delta: u64, object: SubgroupObject) {
        sender.send_raw(delta, object).await.unwrap();
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
