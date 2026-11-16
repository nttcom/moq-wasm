use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    Session, TrackObject,
    fetch::{Fetch as FetchPart, FetchDataReceiver, FetchHandle},
    modules::client::subscription::ReceivedObject,
};

/// The response to one Standalone FETCH (draft-14 §9.16.1).
pub struct Fetch {
    session: Arc<Session>,
    handle: FetchHandle,
    receiver: Mutex<Option<FetchDataReceiver>>,
}

impl Fetch {
    pub(super) fn new(session: Arc<Session>, handle: FetchHandle) -> Self {
        Self {
            session,
            handle,
            receiver: Mutex::new(None),
        }
    }

    /// The next object with a payload; `None` once the response ends. Fails
    /// when the session ends before the response stream arrives, and when
    /// the publisher resets the stream.
    pub async fn next(&self) -> anyhow::Result<Option<ReceivedObject>> {
        let mut receiver = self.receiver.lock().await;
        let receiver = match &mut *receiver {
            Some(receiver) => receiver,
            None => receiver.insert(
                self.session
                    .subscriber()
                    .accept_fetch_receiver(&self.handle)
                    .await?,
            ),
        };
        loop {
            match receiver.receive().await? {
                FetchPart::Object(object) => {
                    if let Some(object) = TrackObject::from_object(object) {
                        return Ok(Some(object.into()));
                    }
                }
                FetchPart::UnknownRange { .. } => {}
                FetchPart::End => return Ok(None),
            }
        }
    }
}
