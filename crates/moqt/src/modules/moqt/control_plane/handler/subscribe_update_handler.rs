use std::sync::{Arc, Mutex};

use thiserror::Error;
use tokio::sync::oneshot;

use crate::{
    modules::moqt::{
        control_plane::control_messages::messages::{
            parameters::filter_type::FilterType, subscribe_update::SubscribeUpdate,
        },
        domains::connection::Connection,
        version::Version,
    },
    object::Location,
};

/// The LARGEST_OBJECT a draft-18 REQUEST_UPDATE is answered with; closed when
/// every handler clone was dropped unanswered.
pub(crate) type UpdateAnswerReceiver = oneshot::Receiver<Option<Location>>;

/// Shared by the handler's clones; the first answer takes the sender.
type UpdateAnswerSender = Arc<Mutex<Option<oneshot::Sender<Option<Location>>>>>;

/// Why the session's draft refuses an update that widens its subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum SubscriptionUpdateRefusal {
    /// draft-14 §9.10: the Start Location must not decrease and the End
    /// Group must not increase; the publisher closes the session with
    /// PROTOCOL_VIOLATION.
    #[error("the update widens the subscription")]
    Widening,
}

#[derive(Clone, Debug)]
pub struct SubscribeUpdateHandler {
    /// `None` for draft-14, whose SUBSCRIBE_UPDATE has no answer.
    answer_sender: Option<UpdateAnswerSender>,
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
        let (answer_sender, answer_receiver) = oneshot::channel();
        let handler = Self {
            answer_sender: Some(Arc::new(Mutex::new(Some(answer_sender)))),
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
            answer_sender: None,
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

    /// Whether the session's draft lets this update widen its subscription,
    /// that is move its Start Location back or its End Group out; the refusal
    /// names the answer its draft requires. draft-14 forbids it (§9.10);
    /// draft-18 allows it, and the subscriber FETCHes what lies before the
    /// Largest Object the answer carries (§10.9.1). Only meaningful for an
    /// update that widens.
    pub fn check_widening(&self) -> Result<(), SubscriptionUpdateRefusal> {
        match self.version {
            Version::Draft14 => Err(SubscriptionUpdateRefusal::Widening),
            Version::Draft18 => Ok(()),
        }
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

#[cfg(test)]
mod tests {
    use super::SubscriptionUpdateRefusal;
    use crate::modules::test_support::{
        published_track, published_track_between, received_subscribe_update,
        spawn_connected_draft18_sessions,
    };

    #[tokio::test]
    async fn a_draft14_update_may_not_widen_its_subscription() {
        // Arrange
        let track = published_track("update-handler-widen-14").await;
        let update = received_subscribe_update(&track).await;

        // Act
        let checked = update.check_widening();

        // Assert: draft-14 §9.10
        assert_eq!(checked, Err(SubscriptionUpdateRefusal::Widening));
    }

    #[tokio::test]
    async fn a_draft18_update_may_widen_its_subscription() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("update-handler-widen-18", "moqt").await;
        let track = published_track_between(client, server).await;
        let update = received_subscribe_update(&track).await;

        // Act
        let checked = update.check_widening();

        // Assert: draft-18 §10.9.1
        assert_eq!(checked, Ok(()));
    }
}
