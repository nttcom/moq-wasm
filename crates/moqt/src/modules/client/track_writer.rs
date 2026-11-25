use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::Semaphore;

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
    subscribers: Arc<Semaphore>,
}

impl TrackWriter {
    pub(super) fn new(routes: Arc<Routes>, namespace: String, track: String) -> Self {
        let subscribers = routes.watch_subscribers(&namespace, &track);
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
    /// (draft-ietf-moq-msf-02 §5). Returns the group's id when a subscriber
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
        match self.subscribers.acquire().await {
            Ok(permit) => {
                permit.forget();
                true
            }
            Err(_) => false,
        }
    }
}

impl Drop for TrackWriter {
    fn drop(&mut self) {
        self.routes
            .unwatch_subscribers(&self.namespace, &self.track, &self.subscribers);
    }
}

fn sent_group(outcome: WriteOutcome) -> Option<u64> {
    match outcome {
        WriteOutcome::Sent(placement) => Some(placement.location.group_id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use futures::FutureExt;

    use super::*;

    fn key() -> (String, String) {
        ("ns".to_string(), "track".to_string())
    }

    #[tokio::test]
    async fn each_answered_subscribe_resolves_one_next_subscriber() {
        // Arrange
        let routes = Arc::new(Routes::default());
        let writer = TrackWriter::new(Arc::clone(&routes), key().0, key().1);

        // Act
        routes.notify_subscribed(&key());
        routes.notify_subscribed(&key());

        // Assert
        assert_eq!(writer.next_subscriber().now_or_never(), Some(true));
        assert_eq!(writer.next_subscriber().now_or_never(), Some(true));
        assert_eq!(writer.next_subscriber().now_or_never(), None);
    }

    #[tokio::test]
    async fn next_subscriber_is_false_once_the_session_ends_or_a_later_writer_takes_over() {
        // Arrange
        let routes = Arc::new(Routes::default());
        let replaced = TrackWriter::new(Arc::clone(&routes), key().0, key().1);
        let ended = TrackWriter::new(Arc::clone(&routes), key().0, key().1);

        // Act
        routes.end();

        // Assert
        assert_eq!(replaced.next_subscriber().now_or_never(), Some(false));
        assert_eq!(ended.next_subscriber().now_or_never(), Some(false));
    }

    #[tokio::test]
    async fn a_dropped_writer_leaves_the_watch_of_the_writer_that_replaced_it() {
        // Arrange
        let routes = Arc::new(Routes::default());
        let replaced = TrackWriter::new(Arc::clone(&routes), key().0, key().1);
        let current = TrackWriter::new(Arc::clone(&routes), key().0, key().1);

        // Act
        drop(replaced);
        routes.notify_subscribed(&key());

        // Assert
        assert_eq!(current.next_subscriber().now_or_never(), Some(true));
        drop(current);
        assert!(routes.subscriber_watchers_is_empty());
    }
}
