use tokio::sync::{mpsc, watch};

use crate::modules::moqt::control_plane::enums::SessionEvent;

/// The events a channel holds unread before the tasks that read the peer's
/// control messages wait for its reader (draft-14 §12.1, draft-18 §13.5).
pub(crate) const MAX_UNREAD_EVENTS: usize = 1024;

pub(crate) fn channel() -> (SessionEventSender, SessionEventReceiver) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let unread = watch::Sender::new(0);
    (
        SessionEventSender {
            sender,
            unread: unread.clone(),
        },
        SessionEventReceiver { receiver, unread },
    )
}

#[derive(Debug)]
pub(crate) struct ReceiverGone;

#[derive(Clone)]
pub(crate) struct SessionEventSender {
    sender: mpsc::UnboundedSender<SessionEvent>,
    unread: watch::Sender<usize>,
}

impl SessionEventSender {
    /// Queues `event` even past `MAX_UNREAD_EVENTS`: the tasks that read the
    /// peer's messages await `has_room` first.
    pub(crate) fn send(&self, event: SessionEvent) -> Result<(), ReceiverGone> {
        self.unread.send_modify(|unread| *unread += 1);
        self.sender.send(event).map_err(|_| ReceiverGone)
    }

    pub(crate) async fn has_room(&self) {
        let mut unread = self.unread.subscribe();
        tokio::select! {
            _ = unread.wait_for(|unread| *unread < MAX_UNREAD_EVENTS) => {}
            () = self.sender.closed() => {}
        }
    }

    #[cfg(test)]
    pub(crate) fn unread(&self) -> usize {
        *self.unread.borrow()
    }
}

pub(crate) struct SessionEventReceiver {
    receiver: mpsc::UnboundedReceiver<SessionEvent>,
    unread: watch::Sender<usize>,
}

impl SessionEventReceiver {
    pub(crate) async fn recv(&mut self) -> Option<SessionEvent> {
        let event = self.receiver.recv().await?;
        self.unread.send_modify(|unread| *unread -= 1);
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use futures::FutureExt;

    use super::*;

    #[tokio::test]
    async fn a_channel_has_room_again_once_an_event_is_read() {
        // Arrange
        let (sender, mut receiver) = channel();
        for _ in 0..MAX_UNREAD_EVENTS {
            sender.send(SessionEvent::Disconnected()).unwrap();
        }

        // Act
        let full = sender.has_room().now_or_never();
        receiver.recv().await.unwrap();
        let read = sender.has_room().now_or_never();

        // Assert
        assert_eq!((full, read), (None, Some(())));
    }

    #[tokio::test]
    async fn a_full_channel_whose_receiver_is_gone_does_not_hold_its_sender() {
        // Arrange
        let (sender, receiver) = channel();
        for _ in 0..MAX_UNREAD_EVENTS {
            sender.send(SessionEvent::Disconnected()).unwrap();
        }

        // Act
        drop(receiver);

        // Assert
        assert_eq!(sender.has_room().now_or_never(), Some(()));
    }
}
