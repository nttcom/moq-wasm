use tokio::sync::oneshot;

use crate::modules::moqt::control_plane::{
    error_codes::RequestErrorCode, handler::shared_answer::SharedAnswer,
    namespace_interest::NamespaceInterest,
};

/// How the application answered a namespace subscription update.
pub(crate) type NamespaceUpdateAnswer = Result<(), (RequestErrorCode, String)>;

/// The answer, with the sender to drop once it has been sent to the peer;
/// closed when every handler clone was dropped unanswered.
pub(crate) type NamespaceUpdateAnswerReceiver =
    oneshot::Receiver<(NamespaceUpdateAnswer, oneshot::Sender<()>)>;

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
/// `RequestErrorCode::NotSupported`. A refused update ends the subscription
/// (§10.9.1), which then raises `UnsubscribeNamespace` for the old prefix.
#[derive(Clone, Debug)]
pub struct SubscribeNamespaceUpdateHandler {
    track_namespace_prefix: String,
    new_track_namespace_prefix: String,
    new_track_namespace_prefix_tuple: Vec<String>,
    interest: NamespaceInterest,
    answer_sender: SharedAnswer<(NamespaceUpdateAnswer, oneshot::Sender<()>)>,
}

impl SubscribeNamespaceUpdateHandler {
    pub(crate) fn new(
        track_namespace_prefix: &[String],
        new_track_namespace_prefix: &[String],
        interest: NamespaceInterest,
    ) -> (Self, NamespaceUpdateAnswerReceiver) {
        let (answer_sender, answer_receiver) = SharedAnswer::new();
        let handler = Self {
            track_namespace_prefix: track_namespace_prefix.join("/"),
            new_track_namespace_prefix: new_track_namespace_prefix.join("/"),
            new_track_namespace_prefix_tuple: new_track_namespace_prefix.to_vec(),
            interest,
            answer_sender,
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
    /// new prefix.
    pub async fn ok(&self) {
        self.answer(Ok(())).await;
    }

    /// Refuses the update; returns once REQUEST_ERROR was sent and the
    /// subscription ended.
    pub async fn error(&self, code: RequestErrorCode, reason_phrase: String) {
        self.answer(Err((code, reason_phrase))).await;
    }

    /// Does nothing after the first answer of any clone, or once the
    /// session is gone.
    async fn answer(&self, answer: NamespaceUpdateAnswer) {
        let (sent_sender, sent_receiver) = oneshot::channel();
        if self.answer_sender.send((answer, sent_sender)) {
            let _ = sent_receiver.await;
        }
    }
}
