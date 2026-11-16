use tokio::sync::Mutex;

use crate::{
    TrackObject,
    fetch::{Fetch as FetchPart, FetchDataReceiver},
    modules::client::subscription::ReceivedObject,
};

/// The response to one Standalone FETCH (draft-14 §9.16.1).
pub struct Fetch {
    receiver: Mutex<FetchDataReceiver>,
}

impl Fetch {
    pub(super) fn new(receiver: FetchDataReceiver) -> Self {
        Self {
            receiver: Mutex::new(receiver),
        }
    }

    /// The next object with a payload; `None` once the response ends. Fails
    /// when the publisher resets the stream.
    pub async fn next(&self) -> anyhow::Result<Option<ReceivedObject>> {
        let mut receiver = self.receiver.lock().await;
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
