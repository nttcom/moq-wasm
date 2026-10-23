use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::oneshot;

/// The answer a handler's clones share: the first `send` delivers it.
#[derive(Debug)]
pub(crate) struct SharedAnswer<T>(Arc<Mutex<Option<oneshot::Sender<T>>>>);

// A derived `Clone` would require `T: Clone`.
impl<T> Clone for SharedAnswer<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> SharedAnswer<T> {
    pub(crate) fn new() -> (Self, oneshot::Receiver<T>) {
        let (sender, receiver) = oneshot::channel();
        (Self(Arc::new(Mutex::new(Some(sender)))), receiver)
    }

    /// `false` after an earlier answer, or once the receiver is gone.
    pub(crate) fn send(&self, answer: T) -> bool {
        let sender = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        sender.is_some_and(|sender| sender.send(answer).is_ok())
    }
}
