use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use tokio::sync::mpsc;

use crate::modules::moqt::data_plane::{
    object::canonical_object::Object, stream::stream_data_receiver::StreamDataReceiver,
};

/// The datagrams queued for a subscription and not read yet: unlike
/// subgroup streams, datagrams take no flow control.
const MAX_QUEUED_DATAGRAMS: usize = 1024;

#[derive(Debug)]
pub(crate) enum IncomingObject {
    Subgroup(StreamDataReceiver),
    Datagram(Object),
}

pub(crate) fn incoming_object_channel() -> (IncomingObjectSender, IncomingObjectReceiver) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let queued_datagrams = Arc::new(AtomicUsize::new(0));
    (
        IncomingObjectSender {
            sender,
            queued_datagrams: queued_datagrams.clone(),
        },
        IncomingObjectReceiver {
            receiver,
            queued_datagrams,
        },
    )
}

#[derive(Clone)]
pub(crate) struct IncomingObjectSender {
    sender: mpsc::UnboundedSender<IncomingObject>,
    queued_datagrams: Arc<AtomicUsize>,
}

pub(crate) enum IncomingObjectSendError {
    /// `MAX_QUEUED_DATAGRAMS` datagrams are queued already.
    Full,
    Closed,
}

impl IncomingObjectSender {
    pub(crate) fn send(&self, object: IncomingObject) -> Result<(), IncomingObjectSendError> {
        if matches!(object, IncomingObject::Datagram(_))
            && self.queued_datagrams.fetch_add(1, Ordering::Relaxed) >= MAX_QUEUED_DATAGRAMS
        {
            self.queued_datagrams.fetch_sub(1, Ordering::Relaxed);
            return Err(IncomingObjectSendError::Full);
        }
        self.sender
            .send(object)
            .map_err(|_| IncomingObjectSendError::Closed)
    }
}

#[derive(Debug)]
pub(crate) struct IncomingObjectReceiver {
    receiver: mpsc::UnboundedReceiver<IncomingObject>,
    queued_datagrams: Arc<AtomicUsize>,
}

impl IncomingObjectReceiver {
    pub(crate) async fn recv(&mut self) -> Option<IncomingObject> {
        let object = self.receiver.recv().await;
        self.count_out(object.as_ref());
        object
    }

    #[cfg(test)]
    pub(crate) fn try_recv(&mut self) -> Result<IncomingObject, mpsc::error::TryRecvError> {
        let object = self.receiver.try_recv();
        self.count_out(object.as_ref().ok());
        object
    }

    fn count_out(&self, object: Option<&IncomingObject>) {
        if let Some(IncomingObject::Datagram(_)) = object {
            self.queued_datagrams.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        IncomingObject, IncomingObjectSendError, IncomingObjectSender, MAX_QUEUED_DATAGRAMS,
        incoming_object_channel,
    };
    use crate::modules::test_support::datagram_object;

    fn send_datagram(sender: &IncomingObjectSender) -> Result<(), IncomingObjectSendError> {
        sender.send(IncomingObject::Datagram(datagram_object()))
    }

    #[test]
    fn a_datagram_beyond_the_unread_limit_is_refused() {
        // Arrange
        let (sender, _receiver) = incoming_object_channel();
        for _ in 0..MAX_QUEUED_DATAGRAMS {
            assert!(send_datagram(&sender).is_ok());
        }

        // Act
        let result = send_datagram(&sender);

        // Assert
        assert!(matches!(result, Err(IncomingObjectSendError::Full)));
    }

    #[test]
    fn reading_a_datagram_makes_room_for_the_next() {
        // Arrange
        let (sender, mut receiver) = incoming_object_channel();
        for _ in 0..MAX_QUEUED_DATAGRAMS {
            assert!(send_datagram(&sender).is_ok());
        }
        receiver.try_recv().unwrap();

        // Act
        let result = send_datagram(&sender);

        // Assert
        assert!(result.is_ok());
    }

    #[test]
    fn a_clone_of_the_sender_shares_the_unread_limit() {
        // Arrange
        let (sender, _receiver) = incoming_object_channel();
        let other_sender = sender.clone();
        for _ in 0..MAX_QUEUED_DATAGRAMS {
            assert!(send_datagram(&sender).is_ok());
        }

        // Act
        let result = send_datagram(&other_sender);

        // Assert
        assert!(matches!(result, Err(IncomingObjectSendError::Full)));
    }
}
