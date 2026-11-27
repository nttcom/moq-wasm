use std::sync::Arc;

use anyhow::{Context, bail};
use bytes::Bytes;
use tokio::sync::{
    Mutex, Semaphore,
    mpsc::{self, error::TryRecvError},
};

use crate::{
    modules::client::routes::Routes,
    object::ExtensionHeaders,
    publish::{GroupBoundary, OutgoingObject, TrackDemand, WriteOutcome},
};

/// A track `Session::publish_track` registered with the session's `Origin`,
/// which numbers and caches what is written and answers the track's
/// SUBSCRIBE, FETCH and TRACK_STATUS.
pub struct TrackWriter {
    routes: Arc<Routes>,
    namespace: String,
    track: String,
    subscribers: Arc<Semaphore>,
    /// `None` for a track published without `PublishTrackOptions::on_demand`.
    demand: Option<Mutex<mpsc::UnboundedReceiver<TrackDemand>>>,
}

impl TrackWriter {
    pub(super) fn new(
        routes: Arc<Routes>,
        namespace: String,
        track: String,
        demand_receiver: Option<mpsc::UnboundedReceiver<TrackDemand>>,
    ) -> Self {
        let subscribers = routes.watch_subscribers(&namespace, &track);
        Self {
            routes,
            namespace,
            track,
            subscribers,
            demand: demand_receiver.map(Mutex::new),
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

    /// Whether a subscription to the track is in Forward State 1 (draft-18
    /// §10.10, draft-14 §5.1), for a track published with
    /// `PublishTrackOptions::on_demand`. The first call returns the current
    /// value; each later call waits for a change and returns the latest
    /// value, which repeats the previous one when the changes between two
    /// calls cancel out. While it is `false`, no object written is sent.
    /// Fails for a track published without `on_demand`, once `end` ended the
    /// track or a later `publish_track` of it with `on_demand` took the
    /// reports over, and once the session has ended.
    pub async fn demand_change(&self) -> anyhow::Result<bool> {
        let demand = self
            .demand
            .as_ref()
            .context("the track was published without on_demand")?;
        let mut ended = self.routes.ended();
        let mut demand_receiver = demand.lock().await;
        let mut latest = tokio::select! {
            biased;
            _ = ended.wait_for(|ended| *ended) => bail!("the session ended"),
            demand = demand_receiver.recv() => demand.context("the track ended")?,
        };
        loop {
            match demand_receiver.try_recv() {
                Ok(demand) => latest = demand,
                Err(TryRecvError::Empty) => return Ok(latest.wanted),
                Err(TryRecvError::Disconnected) => bail!("the track ended"),
            }
        }
    }

    /// Ends every subscription to the track with PUBLISH_DONE (draft-14
    /// §9.12, Track Ended) and has a SUBSCRIBE find no track, until a later
    /// `publish_track` of the same track makes it subscribable again. Fails
    /// when sending a PUBLISH_DONE fails, after every subscription was ended.
    pub async fn end(&self) -> anyhow::Result<()> {
        self.routes
            .origin
            .end_track(&self.namespace, &self.track)
            .await
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
        let writer = TrackWriter::new(Arc::clone(&routes), key().0, key().1, None);

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
        let replaced = TrackWriter::new(Arc::clone(&routes), key().0, key().1, None);
        let ended = TrackWriter::new(Arc::clone(&routes), key().0, key().1, None);

        // Act
        routes.end();

        // Assert
        assert_eq!(replaced.next_subscriber().now_or_never(), Some(false));
        assert_eq!(ended.next_subscriber().now_or_never(), Some(false));
    }

    fn on_demand_writer(routes: &Arc<Routes>) -> (mpsc::UnboundedSender<TrackDemand>, TrackWriter) {
        let (demand_sender, demand_receiver) = mpsc::unbounded_channel();
        let writer = TrackWriter::new(Arc::clone(routes), key().0, key().1, Some(demand_receiver));
        (demand_sender, writer)
    }

    fn demand(wanted: bool) -> TrackDemand {
        TrackDemand {
            track_name: key().1,
            wanted,
        }
    }

    #[tokio::test]
    async fn demand_change_coalesces_the_changes_made_since_the_last_call() {
        // Arrange
        let routes = Arc::new(Routes::default());
        let (demand_sender, writer) = on_demand_writer(&routes);
        for wanted in [false, true, false, true] {
            demand_sender.send(demand(wanted)).unwrap();
        }

        // Act
        let latest = writer.demand_change().now_or_never();
        let next = writer.demand_change().now_or_never();

        // Assert
        assert_eq!(latest.map(|demand| demand.unwrap()), Some(true));
        assert!(next.is_none());
    }

    #[tokio::test]
    async fn demand_change_fails_for_a_track_published_without_on_demand() {
        // Arrange
        let routes = Arc::new(Routes::default());
        let writer = TrackWriter::new(Arc::clone(&routes), key().0, key().1, None);

        // Act
        let demand = writer.demand_change().now_or_never();

        // Assert
        assert!(matches!(demand, Some(Err(_))), "{demand:?}");
    }

    #[tokio::test]
    async fn demand_change_fails_once_the_track_or_the_session_ends() {
        // Arrange
        let track_routes = Arc::new(Routes::default());
        let session_routes = Arc::new(Routes::default());
        let (demand_sender, ended_track) = on_demand_writer(&track_routes);
        let (_session_sender, ended_session) = on_demand_writer(&session_routes);
        demand_sender.send(demand(true)).unwrap();

        // Act: the change still unread when the track ends is not reported
        drop(demand_sender);
        session_routes.end();

        // Assert
        let ended_track = ended_track.demand_change().now_or_never();
        let ended_session = ended_session.demand_change().now_or_never();
        assert!(matches!(ended_track, Some(Err(_))), "{ended_track:?}");
        assert!(matches!(ended_session, Some(Err(_))), "{ended_session:?}");
    }

    #[tokio::test]
    async fn a_dropped_writer_leaves_the_watch_of_the_writer_that_replaced_it() {
        // Arrange
        let routes = Arc::new(Routes::default());
        let replaced = TrackWriter::new(Arc::clone(&routes), key().0, key().1, None);
        let current = TrackWriter::new(Arc::clone(&routes), key().0, key().1, None);

        // Act
        drop(replaced);
        routes.notify_subscribed(&key());

        // Assert
        assert_eq!(current.next_subscriber().now_or_never(), Some(true));
        drop(current);
        assert!(routes.subscriber_watchers_is_empty());
    }
}
