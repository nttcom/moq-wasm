use bytes::{Bytes, BytesMut};
use tokio::{sync::watch, time::Instant};

/// An object whose payload is still arriving on a live stream.
#[derive(Debug)]
pub(crate) struct PartialObject {
    /// The object without its payload.
    pub(crate) object: moqt::object::Object,
    pub(crate) payload_len: usize,
    pub(crate) received_at: Instant,
    progress_sender: watch::Sender<Progress>,
}

/// The pieces received so far and, once the object ended, how it ended.
#[derive(Debug, Default)]
pub(crate) struct Progress {
    pub(crate) pieces: Vec<Bytes>,
    pub(crate) end: Option<PartialEnd>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PartialEnd {
    /// Every payload byte arrived and the complete object is cached.
    Complete,
    /// The upstream stream ended inside the payload.
    Aborted,
}

impl PartialObject {
    pub(crate) fn new(object: moqt::object::Object, payload_len: usize) -> Self {
        Self {
            object,
            payload_len,
            received_at: Instant::now(),
            progress_sender: watch::Sender::new(Progress::default()),
        }
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<Progress> {
        self.progress_sender.subscribe()
    }

    pub(crate) fn append(&self, data: Bytes) {
        self.progress_sender
            .send_modify(|progress| progress.pieces.push(data));
    }

    /// The object with the pieces received so far as its payload.
    pub(crate) fn assemble(&self) -> moqt::object::Object {
        let progress = self.progress_sender.borrow();
        let mut payload = BytesMut::with_capacity(self.payload_len);
        for piece in &progress.pieces {
            payload.extend_from_slice(piece);
        }
        moqt::object::Object {
            payload: payload.freeze(),
            ..self.object.clone()
        }
    }

    /// The first end wins: a completed object is not aborted afterwards.
    pub(crate) fn end(&self, end: PartialEnd) {
        self.progress_sender.send_if_modified(|progress| {
            let unset = progress.end.is_none();
            if unset {
                progress.end = Some(end);
            }
            unset
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{PartialEnd, PartialObject};
    use crate::modules::test_support::relay_harness::fixtures::cached_object::stream_object;

    fn partial() -> PartialObject {
        let mut object = stream_object(0, 0).object.clone();
        object.payload = bytes::Bytes::new();
        PartialObject::new(object, 8)
    }

    #[test]
    fn assemble_joins_the_pieces_in_arrival_order() {
        // Arrange
        let partial = partial();
        partial.append(bytes::Bytes::from_static(b"keyf"));
        partial.append(bytes::Bytes::from_static(b"rame"));

        // Act
        let object = partial.assemble();

        // Assert
        assert_eq!(&object.payload[..], b"keyframe");
        assert_eq!(object.location, partial.object.location);
    }

    #[test]
    fn an_object_that_completed_stays_complete() {
        // Arrange
        let partial = partial();
        let progress = partial.subscribe();
        partial.end(PartialEnd::Complete);

        // Act
        partial.end(PartialEnd::Aborted);

        // Assert
        assert_eq!(progress.borrow().end, Some(PartialEnd::Complete));
    }
}
