use anyhow::bail;
use tokio::sync::oneshot;

use crate::modules::{
    executor,
    moqt::{
        control_plane::error_codes::RequestErrorCode, version::CONTROL_MESSAGE_RESPONSE_TIMEOUT,
    },
};

/// The peer's answer to one REQUEST_UPDATE: REQUEST_OK, or the code and
/// reason of its REQUEST_ERROR (draft-18 §10.9).
pub(crate) type UpdateOutcome = Result<(), (RequestErrorCode, String)>;

pub(crate) type UpdateAnswerSender = oneshot::Sender<UpdateOutcome>;

/// What a sent subscription update waits for before it counts as applied;
/// `None` for a draft-14 SUBSCRIBE_UPDATE, which has no response (§9.10).
pub(crate) struct UpdateAnswer(pub(super) Option<oneshot::Receiver<UpdateOutcome>>);

impl UpdateAnswer {
    /// Resolves once the peer answered the update, and at once for an update
    /// the draft defines no answer to. Fails on REQUEST_ERROR, when the
    /// request stream ended without an answer, or when none came within the
    /// control message timeout.
    pub(crate) async fn wait(self) -> anyhow::Result<()> {
        let Some(receiver) = self.0 else {
            return Ok(());
        };
        match executor::timeout(CONTROL_MESSAGE_RESPONSE_TIMEOUT, receiver).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err((code, reason)))) => {
                bail!("the peer rejected the update with {code:?}: {reason}")
            }
            Ok(Err(_)) => bail!("the subscription ended before the peer answered the update"),
            Err(()) => bail!("the peer did not answer the update in time"),
        }
    }
}
