use std::sync::Arc;

use bytes::{Bytes, BytesMut};

use crate::modules::moqt::{
    data_plane::{
        immutable_properties,
        object::canonical_object::{Object, SubgroupStart},
        object_ledger::{CopySource, ObjectLedger, PayloadHasher},
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

/// One step of a subgroup stream as `SubgroupPartReceiver` reads it.
#[derive(Debug)]
pub enum SubgroupPart {
    Header(SubgroupStart),
    /// An object whose payload arrived with the fields before it, or a
    /// status object.
    Object(Object),
    /// An object whose payload is still arriving: `object.payload` is empty,
    /// and its `payload_len` bytes follow as `Payload` parts. `payload_len`
    /// is the peer's claim, so do not allocate it up front.
    ObjectStart {
        object: Object,
        payload_len: usize,
    },
    /// The next bytes of the current object's payload; `last` ends the
    /// object. `data` shares the stream's read buffer: copy it before keeping
    /// it beyond the object.
    Payload {
        data: Bytes,
        last: bool,
    },
}

#[derive(Debug)]
pub struct StreamDataReceiver {
    pub track_alias: u64,
    reader: SubgroupReader,
    header_returned: bool,
    /// Read ahead for a header that takes its subgroup id from the first
    /// object. Boxed: rare, and inline it would grow every receiver.
    first_part: Option<Box<SubgroupPart>>,
    open: Option<OpenSubgroup>,
    /// The track's ledger once the stream is handed to the application.
    ledger: Option<Arc<ObjectLedger>>,
    /// The object whose payload is arriving, and the payload's hash so far.
    /// Boxed like `first_part`.
    arriving: Option<Box<(Object, PayloadHasher)>>,
}

impl StreamDataReceiver {
    pub(crate) fn new(track_alias: u64, reader: SubgroupReader) -> Self {
        Self {
            track_alias,
            reader,
            header_returned: false,
            first_part: None,
            open: None,
            ledger: None,
            arriving: None,
        }
    }

    /// Checks every object read from here on against the track's `ledger`.
    pub(crate) fn check_with(&mut self, ledger: Arc<ObjectLedger>) {
        self.ledger = Some(ledger);
    }

    pub(crate) fn group_id(&self) -> u64 {
        self.reader.group_id()
    }

    pub(crate) fn inherit_priority(&mut self, default_publisher_priority: u8) {
        self.reader.inherit_priority(default_publisher_priority);
    }

    /// Counts this stream in `open_subgroups` until it ends or is dropped.
    pub(crate) fn count_in(&mut self, open_subgroups: &OpenSubgroups) {
        self.open = Some(open_subgroups.open());
    }

    /// Returns the header first, then each object with absolute ids and its
    /// whole payload. `Ok(None)` means the peer finished the stream (FIN). A
    /// header whose subgroup id comes from the first object is returned only
    /// after that object starts to arrive, so a stream that ends before it
    /// returns `Ok(None)` without a header. An object that repeats one
    /// received before with other properties is
    /// `StreamReceiveError::MalformedTrack`.
    pub async fn receive(&mut self) -> Result<Option<Subgroup>, StreamReceiveError> {
        let Some(part) = self.receive_part().await? else {
            return Ok(None);
        };
        let object = match part {
            SubgroupPart::Header(start) => return Ok(Some(Subgroup::Header(start))),
            SubgroupPart::Object(object) => object,
            SubgroupPart::ObjectStart { object, .. } => Object {
                payload: self.receive_payload().await?,
                ..object
            },
            // `receive` reads every payload it starts up to its last part,
            // and `into_parts` consumes the receiver.
            SubgroupPart::Payload { .. } => unreachable!("a payload part outside an object"),
        };
        Ok(Some(Subgroup::Object(object)))
    }

    pub fn into_parts(self) -> SubgroupPartReceiver {
        SubgroupPartReceiver { receiver: self }
    }

    async fn receive_payload(&mut self) -> Result<Bytes, StreamReceiveError> {
        let mut payload = BytesMut::new();
        loop {
            match self.receive_part().await? {
                Some(SubgroupPart::Payload { data, last }) => {
                    payload.extend_from_slice(&data);
                    if last {
                        return Ok(payload.freeze());
                    }
                }
                // The decoder fails a stream that ends inside a payload, and
                // yields nothing else before the payload's last part.
                other => unreachable!("expected a payload part, got {other:?}"),
            }
        }
    }

    async fn receive_part(&mut self) -> Result<Option<SubgroupPart>, StreamReceiveError> {
        let received = self
            .receive_next()
            .await
            .and_then(|received| self.check(received));
        if !matches!(received, Ok(Some(_))) {
            self.open = None;
        }
        received
    }

    /// Checks an object's Immutable Properties as soon as its head arrives,
    /// and the object against the ledger once its payload is complete; a
    /// payload that arrives in pieces is hashed piece by piece.
    fn check(
        &mut self,
        received: Option<SubgroupPart>,
    ) -> Result<Option<SubgroupPart>, StreamReceiveError> {
        let version = self.reader.version();
        if let Some(SubgroupPart::Object(object) | SubgroupPart::ObjectStart { object, .. }) =
            &received
        {
            immutable_properties::check(version, object)
                .map_err(StreamReceiveError::MalformedTrack)?;
        }
        let Some(ledger) = &self.ledger else {
            return Ok(received);
        };
        let source = CopySource::Subscription(version);
        let checked = match &received {
            Some(SubgroupPart::Object(object)) => ledger.check(object, source),
            Some(SubgroupPart::ObjectStart { object, .. }) => {
                self.arriving = Some(Box::new((object.clone(), ledger.payload_hasher())));
                Ok(())
            }
            Some(SubgroupPart::Payload { data, last }) => {
                // The decoder yields payload parts only after an object start.
                let arriving = self
                    .arriving
                    .as_mut()
                    .expect("a payload part follows its object's start");
                arriving.1.update(data);
                match self.arriving.take_if(|_| *last) {
                    Some(arrived) => {
                        let (object, hasher) = *arrived;
                        ledger.check_hashed(&object, hasher, source)
                    }
                    None => Ok(()),
                }
            }
            Some(SubgroupPart::Header(_)) | None => Ok(()),
        };
        checked.map_err(StreamReceiveError::MalformedTrack)?;
        Ok(received)
    }

    async fn receive_next(&mut self) -> Result<Option<SubgroupPart>, StreamReceiveError> {
        if !self.header_returned {
            if self.reader.start().is_none() {
                let Some(first) = self.reader.next_part().await? else {
                    return Ok(None);
                };
                self.first_part = Some(Box::new(first));
            }
            self.header_returned = true;
            let start = self
                .reader
                .start()
                .expect("the first object has fixed the subgroup id");
            return Ok(Some(SubgroupPart::Header(start)));
        }
        if let Some(first) = self.first_part.take() {
            return Ok(Some(*first));
        }
        self.reader.next_part().await
    }
}

/// Reads a subgroup stream with each object's payload handed over as it
/// arrives, so the object can be forwarded before its last byte.
#[derive(Debug)]
pub struct SubgroupPartReceiver {
    receiver: StreamDataReceiver,
}

impl SubgroupPartReceiver {
    /// Returns the header first, then objects, and for an object whose
    /// payload had not arrived with it, its `ObjectStart` and `Payload` parts
    /// up to the `last` one. `Ok(None)` means the peer finished the stream
    /// (FIN); a stream that ends inside a payload is an error.
    pub async fn receive(&mut self) -> Result<Option<SubgroupPart>, StreamReceiveError> {
        self.receiver.receive_part().await
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::Subgroup;
    use crate::{
        modules::test_support::{
            HANDSHAKE_TIMEOUT, first_subgroup_stream, open_raw_subgroup, published_track,
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
        let mut sender = open_raw_subgroup(&factory, 4, SubgroupId::Value(2)).await;
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
        let mut sender = open_raw_subgroup(&factory, 0, SubgroupId::FirstObjectIdDelta).await;
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
        let mut sender = open_raw_subgroup(&factory, 0, SubgroupId::FirstObjectIdDelta).await;
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
        let mut sender = open_raw_subgroup(&factory, 0, SubgroupId::None).await;
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
        let mut sender = open_raw_subgroup(&factory, 0, SubgroupId::None).await;
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
