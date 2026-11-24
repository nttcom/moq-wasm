use std::{collections::VecDeque, sync::Arc, time::Duration};

use moqt::{
    client::{
        self, GroupStartOptions, ReceivedObject, RequestError, SubscribeOptions, Subscription,
    },
    executor,
};
use msf::CATALOG_TRACK_NAME;
use serde_json::Value;

use super::catalog::Catalog;

const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(1);
const FOLLOW_DELIVERY_TIMEOUT: Duration = Duration::from_secs(2);

/// A namespace another publisher publishes, handed to a `watch_broadcasts`
/// handler.
#[derive(Clone)]
pub struct RemoteBroadcast {
    client: Arc<client::Session>,
    namespace: Vec<String>,
}

/// How `RemoteBroadcast::follow` subscribes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FollowOptions {
    /// The Subscriber Priority (draft-14 §7.1);
    /// `SubscribeOptions::default().priority` by default.
    pub priority: u8,
    /// The DELIVERY TIMEOUT (draft-14 §9.2.1.2), 2 s by default: on a
    /// congested link the relay drops an object that would arrive later.
    /// `None` has every object sent.
    pub delivery_timeout: Option<Duration>,
}

impl Default for FollowOptions {
    fn default() -> Self {
        Self {
            priority: SubscribeOptions::default().priority,
            delivery_timeout: Some(FOLLOW_DELIVERY_TIMEOUT),
        }
    }
}

/// How `RemoteBroadcast::event_timeline` subscribes. `delivery_timeout` is
/// `None` by default, so every record arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventTimelineOptions {
    pub priority: u8,
    pub delivery_timeout: Option<Duration>,
}

impl Default for EventTimelineOptions {
    fn default() -> Self {
        let defaults = GroupStartOptions::default();
        Self {
            priority: defaults.priority,
            delivery_timeout: defaults.delivery_timeout,
        }
    }
}

impl RemoteBroadcast {
    pub(super) fn new(client: Arc<client::Session>, namespace: Vec<String>) -> Self {
        Self { client, namespace }
    }

    pub fn namespace(&self) -> &[String] {
        &self.namespace
    }

    /// Each catalog the publisher writes, starting from the one the relay
    /// holds (a joining FETCH). Fails when the SUBSCRIBE fails.
    pub async fn catalogs(&self) -> Result<Catalogs, RequestError> {
        let subscription = self
            .client
            .subscribe_current_group(
                &self.namespace,
                CATALOG_TRACK_NAME,
                GroupStartOptions::default(),
            )
            .await?;
        Ok(Catalogs {
            subscription: ClosingSubscription(Some(subscription)),
            track_namespace: self.namespace.join("/"),
        })
    }

    /// Subscribe to objects written after subscribe, retrying 1 s after track end or failure.
    pub fn follow(&self, name: &str, options: FollowOptions) -> FollowedTrack {
        FollowedTrack {
            resubscriber: Resubscriber::new(self, name, TrackStart::LargestObject(options)),
        }
    }

    /// Subscribe to an MSF event timeline (`draft-ietf-moq-msf-02` §8) from the current group via joining `FETCH`,
    /// retrying on end/failure like `follow()`.
    pub fn event_timeline(&self, name: &str, options: EventTimelineOptions) -> RemoteEventTimeline {
        RemoteEventTimeline {
            resubscriber: Resubscriber::new(self, name, TrackStart::CurrentGroup(options)),
            records: VecDeque::new(),
            last: None,
        }
    }
}

/// The catalogs of a `RemoteBroadcast`; dropping it unsubscribes.
pub struct Catalogs {
    subscription: ClosingSubscription,
    track_namespace: String,
}

impl Catalogs {
    /// The next catalog; one that does not parse is logged and skipped.
    /// `None` once the catalog track ends.
    pub async fn next(&mut self) -> anyhow::Result<Option<Catalog>> {
        let Some(subscription) = &self.subscription.0 else {
            return Ok(None);
        };
        while let Some(object) = subscription.next().await? {
            match Catalog::parse(&object.payload) {
                Ok(catalog) => return Ok(Some(catalog)),
                Err(error) => {
                    tracing::warn!(%error, track_namespace = self.track_namespace, "catalog ignored")
                }
            }
        }
        Ok(None)
    }
}

/// A followed track, read with `next`; dropping it unsubscribes.
pub struct FollowedTrack {
    resubscriber: Resubscriber,
}

impl FollowedTrack {
    /// The next object. It never ends by itself.
    pub async fn next(&mut self) -> ReceivedObject {
        self.resubscriber.next().await
    }
}

/// The records of an event timeline track, read with `next`; dropping it
/// unsubscribes.
pub struct RemoteEventTimeline {
    resubscriber: Resubscriber,
    records: VecDeque<Value>,
    /// The group and object id of the last object read, so a resubscription
    /// does not yield the records already read.
    last: Option<(u64, u64)>,
}

impl RemoteEventTimeline {
    /// The next record. An object that is not a JSON array is logged and
    /// skipped. It never ends by itself.
    pub async fn next(&mut self) -> Value {
        loop {
            if let Some(record) = self.records.pop_front() {
                return record;
            }
            let object = self.resubscriber.next().await;
            let location = (object.group_id, object.object_id);
            if self.last.is_some_and(|last| location <= last) {
                continue;
            }
            self.last = Some(location);
            match serde_json::from_slice::<Vec<Value>>(&object.payload) {
                Ok(records) => self.records.extend(records),
                Err(error) => tracing::warn!(
                    %error,
                    track_namespace = self.resubscriber.track_namespace(),
                    track_name = self.resubscriber.track_name,
                    "event timeline object ignored"
                ),
            }
        }
    }
}

enum TrackStart {
    LargestObject(FollowOptions),
    CurrentGroup(EventTimelineOptions),
}

/// One track's subscription, made again a second after it ends or fails.
struct Resubscriber {
    client: Arc<client::Session>,
    namespace: Vec<String>,
    track_name: String,
    start: TrackStart,
    subscription: ClosingSubscription,
}

impl Resubscriber {
    fn new(broadcast: &RemoteBroadcast, name: &str, start: TrackStart) -> Self {
        Self {
            client: broadcast.client.clone(),
            namespace: broadcast.namespace.clone(),
            track_name: name.to_string(),
            start,
            subscription: ClosingSubscription(None),
        }
    }

    fn track_namespace(&self) -> String {
        self.namespace.join("/")
    }

    async fn next(&mut self) -> ReceivedObject {
        loop {
            if self.subscription.0.is_none() {
                match self.subscribe().await {
                    Ok(subscription) => self.subscription.0 = Some(subscription),
                    Err(error) => {
                        tracing::info!(%error, track_namespace = self.track_namespace(), track_name = self.track_name, "SUBSCRIBE failed, subscribing again")
                    }
                }
            }
            if let Some(subscription) = &self.subscription.0 {
                match subscription.next().await {
                    Ok(Some(object)) => return object,
                    end => {
                        tracing::info!(error = ?end.err().map(|error| format!("{error:#}")), track_namespace = self.track_namespace(), track_name = self.track_name, "track ended, subscribing again")
                    }
                }
            }
            self.subscription = ClosingSubscription(None);
            executor::sleep(RESUBSCRIBE_DELAY).await;
        }
    }

    async fn subscribe(&self) -> Result<Subscription, RequestError> {
        match self.start {
            TrackStart::LargestObject(options) => {
                let options = SubscribeOptions {
                    priority: options.priority,
                    delivery_timeout: options.delivery_timeout,
                    ..SubscribeOptions::default()
                };
                self.client
                    .subscribe(&self.namespace, &self.track_name, options)
                    .await
            }
            TrackStart::CurrentGroup(options) => {
                let options = GroupStartOptions {
                    priority: options.priority,
                    delivery_timeout: options.delivery_timeout,
                    ..GroupStartOptions::default()
                };
                self.client
                    .subscribe_current_group(&self.namespace, &self.track_name, options)
                    .await
            }
        }
    }
}

/// UNSUBSCRIBEs on a spawned task when dropped.
struct ClosingSubscription(Option<Subscription>);

impl Drop for ClosingSubscription {
    fn drop(&mut self) {
        let Some(subscription) = self.0.take() else {
            return;
        };
        executor::spawn("moq-wasm-unsubscribe", async move {
            if let Err(error) = subscription.close().await {
                tracing::debug!(error = %format!("{error:#}"), "UNSUBSCRIBE failed");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::test_support::{TEST_TIMEOUT, TestRelay};

    fn remote(session: &crate::Session) -> RemoteBroadcast {
        RemoteBroadcast::new(
            session.client().clone(),
            vec!["anon".into(), "remote".into()],
        )
    }

    #[tokio::test]
    async fn catalogs_start_from_the_one_the_relay_holds_and_skip_one_that_does_not_parse() {
        // Arrange
        let relay = TestRelay::spawn("catalogs");
        let session = relay.connect().await;
        let catalog = json!({"version": "draft-02", "tracks": []}).to_string();
        relay
            .write("anon/remote", "catalog", catalog.as_bytes(), true)
            .await;
        let mut catalogs = remote(&session).catalogs().await.unwrap();

        // Act
        let first = tokio::time::timeout(TEST_TIMEOUT, catalogs.next()).await;
        relay
            .write("anon/remote", "catalog", b"not json", true)
            .await;
        relay
            .write("anon/remote", "catalog", catalog.as_bytes(), true)
            .await;
        let second = tokio::time::timeout(TEST_TIMEOUT, catalogs.next()).await;

        // Assert
        assert!(first.unwrap().unwrap().is_some());
        assert!(second.unwrap().unwrap().is_some());
    }

    #[tokio::test]
    async fn follow_yields_the_objects_written_after_it_subscribed() {
        // Arrange
        let relay = TestRelay::spawn("follow");
        let session = relay.connect().await;
        relay.write("anon/remote", "status", b"old", true).await;
        let mut followed = remote(&session).follow("status", FollowOptions::default());

        // Act: write until the subscription receives an object; it starts after the largest object
        let object = tokio::time::timeout(TEST_TIMEOUT, async {
            let next = followed.next();
            tokio::pin!(next);
            loop {
                tokio::select! {
                    object = &mut next => return object,
                    _ = tokio::time::sleep(Duration::from_millis(50)) => {
                        relay.write("anon/remote", "status", b"new", true).await;
                    }
                }
            }
        })
        .await
        .unwrap();

        // Assert
        assert_eq!(object.payload.as_ref(), b"new");
    }

    #[tokio::test]
    async fn an_event_timeline_starts_with_the_records_the_relay_keeps() {
        // Arrange
        let relay = TestRelay::spawn("remote-event-timeline");
        let session = relay.connect().await;
        relay.write("anon/remote", "events", b"[]", true).await;
        relay
            .write("anon/remote", "events", br#"[{"T":1,"data":"a"}]"#, false)
            .await;
        let mut timeline =
            remote(&session).event_timeline("events", EventTimelineOptions::default());

        // Act
        let kept = tokio::time::timeout(TEST_TIMEOUT, timeline.next())
            .await
            .unwrap();
        relay
            .write("anon/remote", "events", br#"[{"T":2,"data":"b"}]"#, false)
            .await;
        let later = tokio::time::timeout(TEST_TIMEOUT, timeline.next())
            .await
            .unwrap();

        // Assert
        assert_eq!(kept, json!({"T": 1, "data": "a"}));
        assert_eq!(later, json!({"T": 2, "data": "b"}));
    }
}
