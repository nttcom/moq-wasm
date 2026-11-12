use super::subscribe::existing_track_largest;
use crate::modules::{
    control_plane::control_message_forwarder::ControlMessageForwarder,
    data_plane::cache::store::TrackCacheStore,
    directory::InMemoryLocalPubSubDirectory,
    domain::{session_id::SessionId, track_key::TrackKey},
};

pub(crate) struct NewGroupRequest;

impl NewGroupRequest {
    /// Passes a subscriber's NEW_GROUP_REQUEST for a track with an
    /// established upstream subscription to its publishers, in a
    /// REQUEST_UPDATE (draft-18 §10.2.13). Nothing is sent for a track
    /// without dynamic groups, nor for a request the track's
    /// `NewGroupRequestGate` does not admit.
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        track_key: &TrackKey,
        new_group_request: u64,
        table: &InMemoryLocalPubSubDirectory,
        cache_store: &TrackCacheStore,
        forwarder: &ControlMessageForwarder,
    ) {
        let Some(track) = table.track_state(track_key) else {
            return;
        };
        let largest_group = existing_track_largest(cache_store, track_key, &track)
            .map(|location| location.group_id);
        let subscriptions =
            table.admit_new_group_request(track_key, new_group_request, largest_group);
        if subscriptions.is_empty() {
            tracing::debug!(
                subscriber_session_id = session_id,
                track_namespace = %track_key.track_namespace,
                track_name = %track_key.track_name,
                new_group_request,
                ?largest_group,
                "NEW_GROUP_REQUEST not forwarded"
            );
            return;
        }
        for (publisher_session_id, subscription) in subscriptions {
            match forwarder
                .new_group_request(publisher_session_id, &subscription, new_group_request)
                .await
            {
                Ok(true) => tracing::info!(
                    subscriber_session_id = session_id,
                    publisher_session_id,
                    track_namespace = %track_key.track_namespace,
                    track_name = %track_key.track_name,
                    request_id = subscription.request_id(),
                    new_group_request,
                    "NEW_GROUP_REQUEST forwarded"
                ),
                Ok(false) => tracing::debug!(
                    publisher_session_id,
                    "NEW_GROUP_REQUEST not forwarded: the publisher's session cannot carry it"
                ),
                Err(error) => tracing::warn!(
                    %error,
                    publisher_session_id,
                    track_namespace = %track_key.track_namespace,
                    track_name = %track_key.track_name,
                    "failed to forward NEW_GROUP_REQUEST"
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use moqt::{
        Session, SessionEvent,
        publish::{GroupBoundary, MIN_GROUP_OBJECTS, Origin, OutgoingObject, Retention},
        subscribe::SubscribeOption,
    };
    use tokio::{sync::Notify, task::JoinHandle};

    use crate::modules::{
        auth::test_support::{app_token, connect_draft18_client, spawn_draft18_relay},
        test_support::relay_clients::{
            NAMESPACE, TRACK, publish_namespace, subscribe_track, within,
        },
    };

    /// A client that announces `NAMESPACE` and serves `TRACK`, a track with
    /// dynamic groups, from `origin`; `requested` is notified when a request
    /// for a new group reaches it, once per group. `request_updates` counts
    /// the REQUEST_UPDATEs it receives, each of which carries such a request.
    struct DynamicGroupPublisher {
        session: Arc<Session>,
        origin: Origin,
        requested: Arc<Notify>,
        request_updates: Arc<AtomicUsize>,
        events: JoinHandle<()>,
    }

    impl DynamicGroupPublisher {
        async fn run(session: Session) -> Self {
            let session = Arc::new(session);
            let origin = Origin::default();
            origin
                .ensure_track(NAMESPACE, TRACK, 0, Retention::NewestOnly)
                .await;
            let requested = Arc::new(Notify::new());
            origin
                .accept_new_group_requests(NAMESPACE, TRACK, requested.clone())
                .await
                .unwrap();
            publish_namespace(&session, NAMESPACE).await;
            let request_updates = Arc::new(AtomicUsize::new(0));
            let events = tokio::spawn({
                let (session, origin) = (session.clone(), origin.clone());
                let request_updates = request_updates.clone();
                async move {
                    while let Ok(event) = session.receive_event().await {
                        if matches!(event, SessionEvent::SubscribeUpdate(_)) {
                            request_updates.fetch_add(1, Ordering::Relaxed);
                        }
                        origin.handle(event).await;
                    }
                }
            });
            Self {
                session,
                origin,
                requested,
                request_updates,
                events,
            }
        }

        /// Sends PUBLISH for the track and writes its first group.
        async fn publish(&self) {
            within(self.origin.publish(&self.session, NAMESPACE, TRACK))
                .await
                .unwrap();
            self.write_group().await;
        }

        /// Writes a group long enough for the origin to end it on request.
        async fn write_group(&self) {
            for object_id in 0..MIN_GROUP_OBJECTS {
                let group = if object_id == 0 {
                    GroupBoundary::Next
                } else {
                    GroupBoundary::Within
                };
                let object = OutgoingObject::plain(group, "a".into());
                self.origin.write(NAMESPACE, TRACK, object).await.unwrap();
            }
        }

        async fn is_asked_for_a_new_group(&self) -> bool {
            tokio::time::timeout(Duration::from_millis(500), self.requested.notified())
                .await
                .is_ok()
        }

        /// The REQUEST_UPDATEs received, after waiting long enough for one
        /// the relay sent to arrive.
        async fn request_updates(&self) -> usize {
            tokio::time::sleep(Duration::from_millis(500)).await;
            self.request_updates.load(Ordering::Relaxed)
        }
    }

    impl Drop for DynamicGroupPublisher {
        fn drop(&mut self) {
            self.events.abort();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_new_group_request_reaches_the_publisher_of_a_published_track() {
        // Arrange
        let relay = spawn_draft18_relay(app_token(Some(""), Some("")));
        let publisher = DynamicGroupPublisher::run(connect_draft18_client(&relay).await).await;
        publisher.publish().await;
        let subscriber = connect_draft18_client(&relay).await;
        let subscription = subscribe_track(&subscriber, SubscribeOption::default()).await;

        // Act
        let sent = subscriber
            .subscriber()
            .request_new_group(&subscription, None)
            .await
            .unwrap();
        let asked = publisher.is_asked_for_a_new_group().await;

        // Assert
        assert!(sent, "the relay did not pass DYNAMIC_GROUPS downstream");
        assert!(asked);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_new_group_request_stays_at_the_relay_while_another_is_outstanding() {
        // Arrange: one subscriber's request went upstream, and the publisher started no group
        let relay = spawn_draft18_relay(app_token(Some(""), Some("")));
        let publisher = DynamicGroupPublisher::run(connect_draft18_client(&relay).await).await;
        publisher.publish().await;
        let subscriber = connect_draft18_client(&relay).await;
        let subscription = subscribe_track(&subscriber, SubscribeOption::default()).await;
        let requester = subscriber.subscriber();
        requester
            .request_new_group(&subscription, None)
            .await
            .unwrap();
        assert!(publisher.is_asked_for_a_new_group().await);

        // Act
        requester
            .request_new_group(&subscription, None)
            .await
            .unwrap();
        let request_updates = publisher.request_updates().await;

        // Assert: only the first request went upstream
        assert_eq!(request_updates, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_upstream_subscribe_carries_the_new_group_request_of_the_first_subscriber() {
        // Arrange: the track is announced but not published, so the relay subscribes to it
        let relay = spawn_draft18_relay(app_token(Some(""), Some("")));
        let publisher = DynamicGroupPublisher::run(connect_draft18_client(&relay).await).await;
        let subscriber = connect_draft18_client(&relay).await;
        let option = SubscribeOption {
            new_group_request: Some(0),
            ..SubscribeOption::default()
        };

        // Act
        let _subscription = subscribe_track(&subscriber, option).await;
        let asked = publisher.is_asked_for_a_new_group().await;
        let request_updates = publisher.request_updates().await;

        // Assert: the SUBSCRIBE carried it, and no REQUEST_UPDATE repeated it
        assert!(asked);
        assert_eq!(request_updates, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_new_group_request_stays_at_the_relay_while_the_one_of_the_upstream_subscribe_is_outstanding()
     {
        // Arrange: the publisher is in a group when the upstream SUBSCRIBE asks it for a new one
        let relay = spawn_draft18_relay(app_token(Some(""), Some("")));
        let publisher = DynamicGroupPublisher::run(connect_draft18_client(&relay).await).await;
        publisher.write_group().await;
        let subscriber = connect_draft18_client(&relay).await;
        let option = SubscribeOption {
            new_group_request: Some(0),
            ..SubscribeOption::default()
        };
        let subscription = subscribe_track(&subscriber, option).await;
        assert!(publisher.is_asked_for_a_new_group().await);

        // Act: the publisher has not started the group yet
        let sent = subscriber
            .subscriber()
            .request_new_group(&subscription, None)
            .await
            .unwrap();
        let request_updates = publisher.request_updates().await;

        // Assert
        assert!(sent, "the relay did not pass DYNAMIC_GROUPS downstream");
        assert_eq!(request_updates, 0);
    }
}
