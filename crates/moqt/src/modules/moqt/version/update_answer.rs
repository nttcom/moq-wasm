use std::{collections::VecDeque, sync::Arc};

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

/// The answer waiters of this side's REQUEST_UPDATEs on one request stream,
/// oldest first. The peer answers each REQUEST_UPDATE with exactly one
/// REQUEST_OK or REQUEST_ERROR, in order (§10.9), so the k-th answer goes to
/// the k-th waiter. That is the waiter of the k-th written update only while
/// each update is enqueued while holding the request stream, just before its
/// write starts, and that write completes even if the caller is dropped.
#[derive(Debug, Default)]
pub(crate) struct UpdateWaiters(std::sync::Mutex<VecDeque<oneshot::Sender<UpdateOutcome>>>);

impl UpdateWaiters {
    /// Queues the waiter of an update about to be written.
    pub(crate) fn enqueue(self: &Arc<Self>) -> QueuedUpdate {
        let (answer_sender, answer) = oneshot::channel();
        self.lock().push_back(answer_sender);
        QueuedUpdate {
            waiters: self.clone(),
            answer,
        }
    }

    /// Hands an answer to the oldest waiter; an answer with no waiter is
    /// ignored.
    pub(crate) fn answer_front(&self, outcome: UpdateOutcome) {
        let waiter = self.lock().pop_front();
        if let Some(waiter) = waiter {
            let _ = waiter.send(outcome);
        }
    }

    fn withdraw_unwritten(&self) {
        self.lock().pop_back();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<oneshot::Sender<UpdateOutcome>>> {
        self.0.lock().expect("draft-18 update waiters poisoned")
    }
}

/// A REQUEST_UPDATE whose waiter is queued while its message is written.
/// It ends `written` or `withdraw_unwritten`; dropped instead (the send
/// future cancelled), its waiter stays queued and takes the answer of its
/// update, which is still written whole.
#[must_use]
pub(crate) struct QueuedUpdate {
    waiters: Arc<UpdateWaiters>,
    answer: oneshot::Receiver<UpdateOutcome>,
}

impl QueuedUpdate {
    /// The whole message was written: the caller waits for its answer.
    pub(crate) fn written(self) -> UpdateAnswer {
        UpdateAnswer(Some(self.answer))
    }

    /// Writing the message failed: removes the last waiter. A failed write
    /// fails every later one on the stream, so each update queued behind
    /// this one withdraws a waiter too.
    pub(crate) fn withdraw_unwritten(self) {
        self.waiters.withdraw_unwritten();
    }
}

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

#[cfg(test)]
mod tests {
    mod properties {
        use std::{collections::VecDeque, sync::Arc};

        use proptest::{collection::vec, prelude::*};
        use tokio::sync::oneshot::error::TryRecvError;

        use super::super::{QueuedUpdate, UpdateAnswer, UpdateWaiters};
        use crate::modules::moqt::control_plane::error_codes::RequestErrorCode;

        /// The model's outbound actions short of a cancelled send, which
        /// `QueuedUpdate` documents as leaving its waiter queued.
        #[derive(Debug, Clone, Copy)]
        enum Op {
            Start,
            WriteDone,
            WriteFail,
            Timeout,
            PeerAnswer,
        }

        fn ops() -> impl Strategy<Value = Vec<Op>> {
            let op = prop_oneof![
                Just(Op::Start),
                Just(Op::WriteDone),
                Just(Op::WriteFail),
                Just(Op::Timeout),
                Just(Op::PeerAnswer),
            ];
            vec(op, 0..48)
        }

        /// The peer's answer to update `update`, told apart by its reason.
        fn answer_to(update: usize) -> Result<(), (RequestErrorCode, String)> {
            Err((RequestErrorCode::InternalError, update.to_string()))
        }

        proptest! {
            /// With updates started one at a time, every caller waiting on an
            /// update gets the answer the peer sent for that update
            /// (`outAnswerReachesItsUpdate`).
            #[test]
            fn every_answer_reaches_the_update_it_answers(ops in ops()) {
                // Arrange
                let waiters = Arc::new(UpdateWaiters::default());
                let mut queued: Option<(usize, QueuedUpdate)> = None;
                let mut waiting: Vec<(usize, UpdateAnswer)> = Vec::new();
                let mut peer_unanswered = VecDeque::new();
                let mut next_update = 0;

                // Act
                for op in ops {
                    match op {
                        Op::Start if queued.is_none() => {
                            queued = Some((next_update, waiters.enqueue()));
                            next_update += 1;
                        }
                        Op::WriteDone => {
                            if let Some((update, queued)) = queued.take() {
                                waiting.push((update, queued.written()));
                                peer_unanswered.push_back(update);
                            }
                        }
                        Op::WriteFail => {
                            if let Some((_, queued)) = queued.take() {
                                queued.withdraw_unwritten();
                            }
                        }
                        Op::Timeout if !waiting.is_empty() => {
                            waiting.remove(0);
                        }
                        Op::PeerAnswer => {
                            if let Some(update) = peer_unanswered.pop_front() {
                                waiters.answer_front(answer_to(update));
                            }
                        }
                        _ => {}
                    }
                }

                // Assert
                for (update, UpdateAnswer(answer)) in waiting {
                    let mut answer = answer.expect("a draft-18 update has an answer");
                    match answer.try_recv() {
                        Ok(outcome) => prop_assert_eq!(outcome, answer_to(update)),
                        Err(TryRecvError::Empty) => {
                            prop_assert!(peer_unanswered.contains(&update));
                        }
                        Err(TryRecvError::Closed) => prop_assert!(false, "update {update} lost its waiter"),
                    }
                }
            }
        }
    }
}
