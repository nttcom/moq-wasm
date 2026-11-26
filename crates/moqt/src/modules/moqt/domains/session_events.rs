use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use tokio::sync::{mpsc, watch};

use crate::{
    SubscriptionEnd,
    modules::{
        extensions::mutex::lock,
        moqt::control_plane::{
            enums::SessionEvent, handler::publish_done_handler::PublishDoneHandler,
        },
    },
};

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
            subscription_ends: SubscriptionEnds::default(),
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
    subscription_ends: SubscriptionEnds,
}

impl SessionEventSender {
    /// Queues `event` even past `MAX_UNREAD_EVENTS`: the tasks that read the
    /// peer's messages await `has_room` first. A PUBLISH_DONE ends its
    /// subscription's `SubscriptionEnd` and Disconnected every one, whether
    /// or not anyone reads the event.
    pub(crate) fn send(&self, event: SessionEvent) -> Result<(), ReceiverGone> {
        match &event {
            SessionEvent::PublishDone(handler) => self.subscription_ends.publish_done(handler),
            SessionEvent::Disconnected() => self.subscription_ends.end_session(),
            _ => {}
        }
        self.unread.send_modify(|unread| *unread += 1);
        self.sender.send(event).map_err(|_| ReceiverGone)
    }

    /// The subscriptions the events sent here end. Only the session's own
    /// channel registers any: a connection's channel carries wire request
    /// ids while the session follows GOAWAY.
    pub(crate) fn subscription_ends(&self) -> &SubscriptionEnds {
        &self.subscription_ends
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

/// The `SubscriptionEnd`s of the subscriptions this side receives, by
/// logical request id, until each ends.
#[derive(Clone, Default)]
pub(crate) struct SubscriptionEnds(Arc<Mutex<Ends>>);

#[derive(Default)]
struct Ends {
    open: HashMap<u64, watch::Sender<Option<PublishDoneHandler>>>,
    session_ended: bool,
}

impl SubscriptionEnds {
    /// Registered before the subscription's first response is accepted, so
    /// before its PUBLISH_DONE is read. Already ended once the session has.
    pub(crate) fn register(&self, request_id: u64) -> SubscriptionEnd {
        let (sender, receiver) = watch::channel(None);
        let mut ends = lock(&self.0);
        if !ends.session_ended {
            ends.open.insert(request_id, sender);
        }
        SubscriptionEnd::new(receiver)
    }

    /// Ends the subscription without a PUBLISH_DONE.
    pub(crate) fn forget(&self, request_id: u64) {
        lock(&self.0).open.remove(&request_id);
    }

    fn publish_done(&self, handler: &PublishDoneHandler) {
        if let Some(end) = lock(&self.0).open.remove(&handler.request_id()) {
            end.send_replace(Some(handler.clone()));
        }
    }

    fn end_session(&self) {
        let mut ends = lock(&self.0);
        ends.session_ended = true;
        ends.open.clear();
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
    async fn a_subscription_end_registered_after_disconnected_has_already_ended() {
        // Arrange
        let (sender, _receiver) = channel();
        sender.send(SessionEvent::Disconnected()).unwrap();

        // Act
        let end = sender.subscription_ends().register(0);

        // Assert
        assert!(matches!(end.done().now_or_never(), Some(None)));
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
