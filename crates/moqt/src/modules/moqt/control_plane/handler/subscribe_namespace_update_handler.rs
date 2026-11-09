use tokio::sync::oneshot;

use crate::modules::moqt::control_plane::{
    error_codes::RequestErrorCode,
    handler::update_answer::{self, SlotOutcome, UpdateAnswerSlot},
    namespace_interest::NamespaceInterest,
};

/// How the application answered a namespace subscription update.
pub(crate) type PrefixUpdateDecision = Result<(), (RequestErrorCode, String)>;

/// The decision, with the sender the request stream signals on once it has
/// moved the subscription to the new prefix and answered REQUEST_OK.
pub(crate) type NamespaceUpdateAnswerReceiver =
    update_answer::UpdateAnswerReceiver<(PrefixUpdateDecision, oneshot::Sender<()>)>;

/// How `SubscribeNamespaceUpdateHandler::ok` answered the update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PrefixUpdateAnswer {
    /// The subscription has the new prefix.
    Accepted,
    /// The update is not applied: the application did not answer within the
    /// control message timeout, so it was answered REQUEST_ERROR TIMEOUT, or
    /// the subscription ended first. Either way the subscription ended
    /// (draft-18 §10.9.1), and `UnsubscribeNamespace` reports it for the old
    /// prefix.
    Failed,
}

/// The peer moved the Track Namespace Prefix of a namespace subscription it
/// holds: a draft-18 REQUEST_UPDATE with TRACK_NAMESPACE_PREFIX on its
/// SUBSCRIBE_NAMESPACE (`NamespaceInterest::Namespaces`) or SUBSCRIBE_TRACKS
/// (`NamespaceInterest::Tracks`) (draft-18 §10.9.2). draft-14 has no such
/// update.
///
/// The new prefix must not share a common
/// prefix with another subscription of the same interest in the session;
/// refuse such an update with `RequestErrorCode::NamespacePrefixOverlap`
/// (§10.2.14). Dropping every clone unanswered refuses it with
/// `RequestErrorCode::NotSupported`, and leaving it unanswered for the
/// control message timeout with `RequestErrorCode::Timeout`. A refused
/// update ends the subscription (§10.9.1), which then raises
/// `UnsubscribeNamespace` for the old prefix.
///
/// The stream goes on being read while an update awaits its answer, so a
/// later update can arrive first; its old prefix is the one this update asks
/// for. Answers go out in the order the updates arrived, so answering a
/// later one waits for this one.
#[derive(Clone, Debug)]
pub struct SubscribeNamespaceUpdateHandler {
    track_namespace_prefix: String,
    new_track_namespace_prefix: String,
    new_track_namespace_prefix_tuple: Vec<String>,
    interest: NamespaceInterest,
    answer_slot: UpdateAnswerSlot<(PrefixUpdateDecision, oneshot::Sender<()>)>,
}

impl SubscribeNamespaceUpdateHandler {
    pub(crate) fn new(
        track_namespace_prefix: &[String],
        new_track_namespace_prefix: &[String],
        interest: NamespaceInterest,
    ) -> (Self, NamespaceUpdateAnswerReceiver) {
        let (answer_slot, answer_receiver) = UpdateAnswerSlot::new();
        let handler = Self {
            track_namespace_prefix: track_namespace_prefix.join("/"),
            new_track_namespace_prefix: new_track_namespace_prefix.join("/"),
            new_track_namespace_prefix_tuple: new_track_namespace_prefix.to_vec(),
            interest,
            answer_slot,
        };
        (handler, answer_receiver)
    }

    /// The prefix the subscription has until the update is accepted.
    pub fn track_namespace_prefix(&self) -> &str {
        &self.track_namespace_prefix
    }

    pub fn new_track_namespace_prefix(&self) -> &str {
        &self.new_track_namespace_prefix
    }

    pub fn new_track_namespace_prefix_tuple(&self) -> &[String] {
        &self.new_track_namespace_prefix_tuple
    }

    pub fn interest(&self) -> NamespaceInterest {
        self.interest
    }

    /// Accepts the update; returns once REQUEST_OK was sent, so every
    /// namespace announced on the subscription afterwards is relative to the
    /// new prefix. Returns `PrefixUpdateAnswer::Accepted` after an earlier
    /// answer of a clone.
    pub async fn ok(&self) -> PrefixUpdateAnswer {
        match self.answer(Ok(())).await {
            Some(true) | None => PrefixUpdateAnswer::Accepted,
            Some(false) => PrefixUpdateAnswer::Failed,
        }
    }

    /// Refuses the update; returns once REQUEST_ERROR was sent and the
    /// subscription ended. Does nothing after the first answer of any clone,
    /// or once the subscription ended.
    pub async fn error(&self, code: RequestErrorCode, reason_phrase: String) {
        self.answer(Err((code, reason_phrase))).await;
    }

    /// Whether the request stream moved the subscription; `None` after an
    /// earlier answer of a clone.
    async fn answer(&self, decision: PrefixUpdateDecision) -> Option<bool> {
        let (moved_sender, moved_receiver) = oneshot::channel();
        match self.answer_slot.answer((decision, moved_sender)) {
            SlotOutcome::Delivered => Some(moved_receiver.await.is_ok()),
            SlotOutcome::AlreadyAnswered => None,
            SlotOutcome::Undelivered | SlotOutcome::TimedOut => Some(false),
        }
    }
}
