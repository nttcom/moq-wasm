use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

use crate::{
    modules::moqt::{
        control_plane::control_messages::messages::subscribe_update::SubscribeUpdate,
        domains::connection::Connection,
    },
    object::Location,
};

/// The LARGEST_OBJECT a draft-18 REQUEST_UPDATE is answered with; closed when
/// every handler clone was dropped unanswered.
pub(crate) type UpdateAnswerReceiver = oneshot::Receiver<Option<Location>>;

/// Shared by the handler's clones; the first answer takes the sender.
type UpdateAnswerSender = Arc<Mutex<Option<oneshot::Sender<Option<Location>>>>>;

#[derive(Clone, Debug)]
pub struct SubscribeUpdateHandler {
    /// `None` for draft-14, whose SUBSCRIBE_UPDATE has no answer.
    answer_sender: Option<UpdateAnswerSender>,
    request_id: u64,
    subscription_request_id: u64,
    start_location: Location,
    end_group: u64,
    subscriber_priority: u8,
    forward: bool,
    delivery_timeout: Option<u64>,
}

impl SubscribeUpdateHandler {
    pub(crate) fn new(connection: &Connection, subscribe_update: SubscribeUpdate) -> Self {
        Self {
            answer_sender: None,
            request_id: connection.logical_request_id(subscribe_update.request_id),
            subscription_request_id: connection
                .logical_request_id(subscribe_update.subscription_request_id),
            start_location: subscribe_update.start_location,
            end_group: subscribe_update.end_group,
            subscriber_priority: subscribe_update.subscriber_priority,
            forward: subscribe_update.forward,
            delivery_timeout: subscribe_update.delivery_timeout,
        }
    }

    /// For a draft-18 REQUEST_UPDATE, which the receiver answers (§10.9).
    pub(crate) fn answered(
        connection: &Connection,
        subscribe_update: SubscribeUpdate,
    ) -> (Self, UpdateAnswerReceiver) {
        let (answer_sender, answer_receiver) = oneshot::channel();
        let handler = Self {
            answer_sender: Some(Arc::new(Mutex::new(Some(answer_sender)))),
            ..Self::new(connection, subscribe_update)
        };
        (handler, answer_receiver)
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    pub fn subscription_request_id(&self) -> u64 {
        self.subscription_request_id
    }

    pub fn start_location(&self) -> Location {
        self.start_location
    }

    pub fn end_group(&self) -> u64 {
        self.end_group
    }

    pub fn subscriber_priority(&self) -> u8 {
        self.subscriber_priority
    }

    pub fn forward(&self) -> bool {
        self.forward
    }

    pub fn delivery_timeout(&self) -> Option<u64> {
        self.delivery_timeout
    }

    /// Accepts the update. draft-18 answers it with REQUEST_OK carrying
    /// `largest` as LARGEST_OBJECT (§10.2.11: required once the track has
    /// objects); dropping every clone unanswered sends it without. The
    /// subscription's request stream reads nothing else until then. No-op for
    /// draft-14, whose SUBSCRIBE_UPDATE has no answer.
    ///
    /// Returns whether this call gave the peer `largest`. Only a `largest` so
    /// given by an update that turns Forward on becomes the subscription's
    /// Joining Location (draft-18 §5.1); draft-14 joins at the SUBSCRIBE_OK's
    /// Largest Location (§9.16.2).
    pub fn ok(&self, largest: Option<Location>) -> bool {
        let Some(answer_sender) = &self.answer_sender else {
            return false;
        };
        let answer_sender = answer_sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        answer_sender.is_some_and(|answer_sender| answer_sender.send(largest).is_ok())
    }
}
