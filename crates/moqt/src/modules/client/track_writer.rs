use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::{Mutex, mpsc};

use crate::{
    modules::client::routes::Routes,
    object::ExtensionHeaders,
    publish::{GroupBoundary, OutgoingObject, WriteOutcome},
};

/// A track `Session::publish_track` registered with the session's `Origin`,
/// which numbers and caches what is written and answers the track's
/// SUBSCRIBE, FETCH and TRACK_STATUS.
pub struct TrackWriter {
    routes: Arc<Routes>,
    namespace: String,
    track: String,
    subscribers: Mutex<mpsc::UnboundedReceiver<()>>,
}

impl TrackWriter {
    pub(super) fn new(routes: Arc<Routes>, namespace: String, track: String) -> Self {
        let subscribers = Mutex::new(routes.watch_subscribers(&namespace, &track));
        Self {
            routes,
            namespace,
            track,
            subscribers,
        }
    }

    /// Writes one object, starting a new group with it when `new_group`.
    /// `extension_headers` carry, for instance, the object's LOC properties
    /// (draft-ietf-moq-loc-04). Returns the object's group id when a
    /// subscriber took it, and `None` otherwise; it is cached either way. An
    /// object within a group that was never started is dropped.
    pub async fn write(
        &self,
        payload: Bytes,
        new_group: bool,
        extension_headers: ExtensionHeaders,
    ) -> anyhow::Result<Option<u64>> {
        let group = if new_group {
            GroupBoundary::Next
        } else {
            GroupBoundary::Within
        };
        let object = OutgoingObject {
            group,
            extension_headers,
            payload,
        };
        let outcome = self
            .routes
            .origin
            .write(&self.namespace, &self.track, object)
            .await?;
        Ok(sent_group(outcome))
    }

    /// Sends `payload` as a group of its own, the layout of an MSF catalog
    /// (draft-ietf-moq-msf-01 §5). Returns the group's id when a subscriber
    /// took it, and `None` otherwise.
    pub async fn write_group(&self, payload: Bytes) -> anyhow::Result<Option<u64>> {
        let outcome = self
            .routes
            .origin
            .write_group(&self.namespace, &self.track, payload)
            .await?;
        Ok(sent_group(outcome))
    }

    /// Returns `true` once a SUBSCRIBE for the track is answered, and
    /// `false` once the session has ended: the moment to send what a new
    /// subscriber needs, since its SUBSCRIBE starts after the largest
    /// object. A later `publish_track` of the same track takes the
    /// notifications over.
    pub async fn next_subscriber(&self) -> bool {
        self.subscribers.lock().await.recv().await.is_some()
    }
}

fn sent_group(outcome: WriteOutcome) -> Option<u64> {
    match outcome {
        WriteOutcome::Sent(placement) => Some(placement.location.group_id),
        _ => None,
    }
}
