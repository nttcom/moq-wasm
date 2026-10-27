use std::{
    sync::{Arc, Mutex, PoisonError, Weak},
    time::Duration,
};

use tokio::sync::oneshot;

use crate::modules::executor;

/// The answer to a draft-18 REQUEST_UPDATE, shared by the handler's clones.
/// Dropping every clone drops the sender, which the request stream reads as
/// `DueAnswer::Dropped`.
enum AnswerSlot<T> {
    Waiting(oneshot::Sender<T>),
    Answered,
    /// Answered REQUEST_ERROR because the application did not answer in time.
    TimedOut,
}

impl<T> std::fmt::Debug for AnswerSlot<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Waiting(_) => "Waiting",
            Self::Answered => "Answered",
            Self::TimedOut => "TimedOut",
        })
    }
}

/// The handler's side of an update's answer.
#[derive(Debug)]
pub(crate) struct UpdateAnswerSlot<T>(Arc<Mutex<AnswerSlot<T>>>);

// A derived `Clone` would require `T: Clone`.
impl<T> Clone for UpdateAnswerSlot<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

/// What became of an answer handed to `UpdateAnswerSlot::answer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotOutcome {
    /// The request stream will write it.
    Delivered,
    /// A clone answered first.
    AlreadyAnswered,
    /// The request stream ended first.
    Undelivered,
    /// The update was answered REQUEST_ERROR TIMEOUT first.
    TimedOut,
}

impl<T> UpdateAnswerSlot<T> {
    pub(crate) fn new() -> (Self, UpdateAnswerReceiver<T>) {
        let (sender, receiver) = oneshot::channel();
        let slot = Arc::new(Mutex::new(AnswerSlot::Waiting(sender)));
        let answer_receiver = UpdateAnswerReceiver {
            receiver,
            slot: Arc::downgrade(&slot),
        };
        (Self(slot), answer_receiver)
    }

    pub(crate) fn answer(&self, answer: T) -> SlotOutcome {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if matches!(*slot, AnswerSlot::TimedOut) {
            return SlotOutcome::TimedOut;
        }
        let AnswerSlot::Waiting(sender) = std::mem::replace(&mut *slot, AnswerSlot::Answered)
        else {
            return SlotOutcome::AlreadyAnswered;
        };
        match sender.send(answer) {
            Ok(()) => SlotOutcome::Delivered,
            Err(_) => SlotOutcome::Undelivered,
        }
    }
}

/// What the request stream writes for an update.
#[derive(Debug)]
pub(crate) enum DueAnswer<T> {
    Answered(T),
    /// Every handler clone was dropped unanswered.
    Dropped,
    /// The application did not answer within the timeout; a later answer
    /// gets `SlotOutcome::TimedOut`.
    TimedOut,
}

/// The request stream's side of an update's answer.
pub(crate) struct UpdateAnswerReceiver<T> {
    receiver: oneshot::Receiver<T>,
    slot: Weak<Mutex<AnswerSlot<T>>>,
}

impl<T> UpdateAnswerReceiver<T> {
    pub(crate) async fn within(mut self, timeout: Duration) -> DueAnswer<T> {
        if let Ok(answer) = executor::timeout(timeout, &mut self.receiver).await {
            return answer.map_or(DueAnswer::Dropped, DueAnswer::Answered);
        }
        if let Some(slot) = self.slot.upgrade() {
            let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(*slot, AnswerSlot::Waiting(_)) {
                *slot = AnswerSlot::TimedOut;
                return DueAnswer::TimedOut;
            }
        }
        // Answered, or every clone dropped, as the timeout fired.
        self.receiver
            .try_recv()
            .map_or(DueAnswer::Dropped, DueAnswer::Answered)
    }
}
