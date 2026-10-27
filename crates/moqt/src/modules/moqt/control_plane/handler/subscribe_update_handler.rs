use std::{
    sync::{Arc, Mutex, PoisonError, Weak},
    time::Duration,
};

use tokio::sync::oneshot;

use crate::{
    modules::{
        executor,
        moqt::{
            control_plane::control_messages::messages::{
                parameters::filter_type::FilterType, subscribe_update::SubscribeUpdate,
            },
            domains::connection::Connection,
            version::Version,
        },
    },
    object::Location,
};

/// The answer of a draft-18 REQUEST_UPDATE, shared by the handler's clones.
/// Dropping every clone drops the sender, which answers without a
/// LARGEST_OBJECT.
#[derive(Debug)]
enum AnswerSlot {
    Waiting(oneshot::Sender<Option<Location>>),
    Answered,
    /// Answered REQUEST_ERROR because the application did not answer in time.
    TimedOut,
}

type SharedAnswerSlot = Arc<Mutex<AnswerSlot>>;

/// The LARGEST_OBJECT a draft-18 REQUEST_UPDATE is answered with.
pub(crate) struct UpdateAnswerReceiver {
    receiver: oneshot::Receiver<Option<Location>>,
    slot: Weak<Mutex<AnswerSlot>>,
}

impl UpdateAnswerReceiver {
    /// The LARGEST_OBJECT the application answered with, `None` inside when
    /// every handler clone was dropped unanswered. `None` when it did not
    /// answer within `timeout`; a later `ok` then reports
    /// `UpdateAnswer::Failed`.
    pub(crate) async fn within(mut self, timeout: Duration) -> Option<Option<Location>> {
        if let Ok(answer) = executor::timeout(timeout, &mut self.receiver).await {
            return Some(answer.unwrap_or(None));
        }
        if let Some(slot) = self.slot.upgrade() {
            let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(*slot, AnswerSlot::Waiting(_)) {
                *slot = AnswerSlot::TimedOut;
                return None;
            }
        }
        // Answered, or every clone dropped, as the timeout fired.
        Some(self.receiver.try_recv().unwrap_or(None))
    }
}

/// How `SubscribeUpdateHandler::ok` answered the update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpdateAnswer {
    /// The update is accepted. `largest_told` is whether the peer was given
    /// the Largest Object: only one so given by an update that turns Forward
    /// on becomes the subscription's Joining Location (draft-18 §5.1);
    /// draft-14 joins at the SUBSCRIBE_OK's Largest Location (§9.16.2).
    Accepted { largest_told: bool },
    /// The update was answered REQUEST_ERROR because the application did not
    /// answer in time. draft-18 §10.9.1: the publisher MUST then end the
    /// subscription with PUBLISH_DONE UPDATE_FAILED; the update is not
    /// applied.
    Failed,
}

#[derive(Clone, Debug)]
pub struct SubscribeUpdateHandler {
    /// `None` for draft-14, whose SUBSCRIBE_UPDATE has no answer.
    answer_slot: Option<SharedAnswerSlot>,
    version: Version,
    request_id: u64,
    subscription_request_id: u64,
    filter: Option<FilterType>,
    subscriber_priority: u8,
    forward: bool,
    delivery_timeout: Option<u64>,
}

impl SubscribeUpdateHandler {
    /// For a draft-14 SUBSCRIBE_UPDATE, which states the whole range (§9.10).
    pub(crate) fn new(connection: &Connection, subscribe_update: SubscribeUpdate) -> Self {
        let filter = match subscribe_update.end_group {
            0 => FilterType::AbsoluteStart {
                location: subscribe_update.start_location,
            },
            end_group => FilterType::AbsoluteRange {
                location: subscribe_update.start_location,
                end_group: end_group - 1,
            },
        };
        Self::with_filter(connection, subscribe_update, Some(filter))
    }

    /// For a draft-18 REQUEST_UPDATE, which the receiver answers (§10.9).
    /// `filter` is its SUBSCRIPTION_FILTER, `None` when it carries none.
    pub(crate) fn answered(
        connection: &Connection,
        subscribe_update: SubscribeUpdate,
        filter: Option<FilterType>,
    ) -> (Self, UpdateAnswerReceiver) {
        let (answer_sender, receiver) = oneshot::channel();
        let slot = Arc::new(Mutex::new(AnswerSlot::Waiting(answer_sender)));
        let answer_receiver = UpdateAnswerReceiver {
            receiver,
            slot: Arc::downgrade(&slot),
        };
        let handler = Self {
            answer_slot: Some(slot),
            ..Self::with_filter(connection, subscribe_update, filter)
        };
        (handler, answer_receiver)
    }

    fn with_filter(
        connection: &Connection,
        subscribe_update: SubscribeUpdate,
        filter: Option<FilterType>,
    ) -> Self {
        Self {
            answer_slot: None,
            version: connection.control.version(),
            request_id: connection.logical_request_id(subscribe_update.request_id),
            subscription_request_id: connection
                .logical_request_id(subscribe_update.subscription_request_id),
            filter,
            subscriber_priority: subscribe_update.subscriber_priority,
            forward: subscribe_update.forward,
            delivery_timeout: subscribe_update.delivery_timeout,
        }
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    pub fn subscription_request_id(&self) -> u64 {
        self.subscription_request_id
    }

    /// The subscription filter the update asks for; `None` keeps the current
    /// one (draft-18 §10.2.9: an omitted SUBSCRIPTION_FILTER is unchanged).
    /// A draft-14 SUBSCRIBE_UPDATE always states its range (§9.10), as an
    /// AbsoluteStart or AbsoluteRange filter. A relative filter is resolved
    /// against the publisher's Largest Object, as for a new subscription.
    pub fn filter(&self) -> Option<FilterType> {
        self.filter
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

    /// Whether the session's draft lets the update move the Start Location
    /// back or the End Group out: draft-14 refuses, and the publisher closes
    /// the session with PROTOCOL_VIOLATION (§9.10); draft-18 allows it
    /// (§10.9.1).
    pub fn may_widen(&self) -> bool {
        match self.version {
            Version::Draft14 => false,
            Version::Draft18 => true,
        }
    }

    /// Accepts the update. draft-18 answers it with REQUEST_OK carrying
    /// `largest` as LARGEST_OBJECT (§10.2.11: required once the track has
    /// objects); dropping every clone unanswered sends it without. An update
    /// left unanswered for the control message timeout is answered
    /// REQUEST_ERROR TIMEOUT instead, and this returns
    /// `UpdateAnswer::Failed`. Answers go out in the order the updates
    /// arrived, so a later one waits for this one. No-op for draft-14, whose
    /// SUBSCRIBE_UPDATE has no answer.
    pub fn ok(&self, largest: Option<Location>) -> UpdateAnswer {
        let Some(slot) = &self.answer_slot else {
            return UpdateAnswer::Accepted {
                largest_told: false,
            };
        };
        let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
        match std::mem::replace(&mut *slot, AnswerSlot::Answered) {
            AnswerSlot::Waiting(answer_sender) => UpdateAnswer::Accepted {
                largest_told: answer_sender.send(largest).is_ok(),
            },
            AnswerSlot::Answered => UpdateAnswer::Accepted {
                largest_told: false,
            },
            AnswerSlot::TimedOut => {
                *slot = AnswerSlot::TimedOut;
                UpdateAnswer::Failed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::modules::test_support::{
        published_track, published_track_between, received_subscribe_update,
        spawn_connected_draft18_sessions,
    };

    #[tokio::test]
    async fn a_draft14_update_may_not_widen_its_subscription() {
        // Arrange
        let track = published_track("update-handler-widen-14").await;
        let update = received_subscribe_update(&track).await;

        // Act / Assert: draft-14 §9.10
        assert!(!update.may_widen());
    }

    #[tokio::test]
    async fn a_draft18_update_may_widen_its_subscription() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("update-handler-widen-18", "moqt").await;
        let track = published_track_between(client, server).await;
        let update = received_subscribe_update(&track).await;

        // Act / Assert: draft-18 §10.9.1
        assert!(update.may_widen());
    }
}
