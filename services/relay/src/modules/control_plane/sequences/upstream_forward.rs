use crate::modules::{
    control_plane::control_message_forwarder::ControlMessageForwarder,
    directory::InMemoryLocalPubSubDirectory,
    domain::{session_id::SessionId, track_key::TrackKey},
};

/// Moves a publisher's upstream subscription to the Forward State
/// `Track::upstream_forward` gives. Runs in the publisher's session worker,
/// after the PUBLISH_OK that established the subscription.
pub(crate) struct UpstreamForward;

impl UpstreamForward {
    pub(crate) async fn handle(
        &self,
        publisher_session_id: SessionId,
        track_key: &TrackKey,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
    ) {
        let Some((subscription, forward)) =
            table.take_upstream_forward_update(track_key, publisher_session_id)
        else {
            return;
        };
        match forwarder
            .update_forward(publisher_session_id, &subscription, forward)
            .await
        {
            Ok(()) => tracing::info!(
                publisher_session_id,
                track_namespace = %track_key.track_namespace,
                track_name = %track_key.track_name,
                request_id = subscription.request_id(),
                forward,
                "upstream Forward State updated"
            ),
            Err(error) => tracing::warn!(
                %error,
                publisher_session_id,
                track_namespace = %track_key.track_namespace,
                track_name = %track_key.track_name,
                request_id = subscription.request_id(),
                forward,
                "failed to update the upstream Forward State"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use moqt::{
        Session, SessionEvent, Subscription, publish::PublishOption, subscribe::SubscribeOption,
    };

    use crate::modules::{
        auth::test_support::{
            app_token, connect_client, connect_draft18_client, spawn_draft18_relay,
            spawn_relay_with_verifier,
        },
        test_support::relay_clients::{NAMESPACE, TRACK, next_event, subscribe_track, within},
    };

    /// Sends PUBLISH for the track in Forward State `forward` and returns
    /// the Forward State its PUBLISH_OK gave the subscription.
    async fn publish_track(publisher: &Session, forward: bool) -> bool {
        let option = PublishOption {
            forward,
            ..PublishOption::default()
        };
        let published = within(publisher.publisher().publish(
            NAMESPACE.to_string(),
            TRACK.to_string(),
            option,
        ))
        .await
        .unwrap();
        matches!(published, Subscription::PublisherInitiated(published) if published.forward)
    }

    async fn next_forward_update(publisher: &Session) -> bool {
        loop {
            if let SessionEvent::SubscribeUpdate(handler) = next_event(publisher).await {
                return handler.forward();
            }
        }
    }

    async fn is_sent_an_update(publisher: &Session) -> bool {
        tokio::time::timeout(Duration::from_millis(300), next_forward_update(publisher))
            .await
            .is_ok()
    }

    /// The Forward State of the track `publisher` publishes on demand: in its
    /// PUBLISH_OK, once `subscriber` subscribed, and once it unsubscribed.
    async fn forward_states(publisher: &Session, subscriber: &Session) -> [bool; 3] {
        let published = publish_track(publisher, false).await;
        let subscription = subscribe_track(subscriber, SubscribeOption::default()).await;
        let with_subscriber = next_forward_update(publisher).await;
        within(subscriber.subscriber().unsubscribe(&subscription))
            .await
            .unwrap();
        let without_subscriber = next_forward_update(publisher).await;
        [published, with_subscriber, without_subscriber]
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_track_published_on_demand_is_forwarded_only_while_a_subscriber_takes_it() {
        // Arrange
        let relay = spawn_draft18_relay(app_token(Some(""), Some("")));
        let publisher = connect_draft18_client(&relay).await;
        let subscriber = connect_draft18_client(&relay).await;

        // Act
        let forward = forward_states(&publisher, &subscriber).await;

        // Assert
        assert_eq!(forward, [false, true, false]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_draft14_track_published_on_demand_is_forwarded_only_while_a_subscriber_takes_it() {
        // Arrange
        let relay = spawn_relay_with_verifier(app_token(Some(""), Some(""))).await;
        let publisher = connect_client(&relay).await;
        let subscriber = connect_client(&relay).await;

        // Act
        let forward = forward_states(&publisher, &subscriber).await;

        // Assert
        assert_eq!(forward, [false, true, false]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_subscriber_in_forward_state_0_does_not_make_the_publisher_send() {
        // Arrange
        let relay = spawn_draft18_relay(app_token(Some(""), Some("")));
        let publisher = connect_draft18_client(&relay).await;
        let subscriber = connect_draft18_client(&relay).await;
        publish_track(&publisher, false).await;
        let option = SubscribeOption {
            forward: false,
            ..SubscribeOption::default()
        };

        // Act
        let subscription = subscribe_track(&subscriber, option).await;
        let updated_for_forward_0 = is_sent_an_update(&publisher).await;
        within(subscriber.subscriber().update_forward(&subscription, true))
            .await
            .unwrap();
        let forward_after_update = next_forward_update(&publisher).await;

        // Assert
        assert!(!updated_for_forward_0);
        assert!(forward_after_update);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_publish_of_a_track_a_subscriber_takes_starts_forwarded() {
        // Arrange: the subscriber already takes the track from an earlier publisher
        let relay = spawn_draft18_relay(app_token(Some(""), Some("")));
        let first_publisher = connect_draft18_client(&relay).await;
        let subscriber = connect_draft18_client(&relay).await;
        publish_track(&first_publisher, false).await;
        let _subscription = subscribe_track(&subscriber, SubscribeOption::default()).await;
        let second_publisher = connect_draft18_client(&relay).await;

        // Act
        let published_forward = publish_track(&second_publisher, false).await;

        // Assert: draft-18 §9.5
        assert!(published_forward);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_track_published_in_forward_state_1_stays_forwarded_without_a_subscriber() {
        // Arrange
        let relay = spawn_draft18_relay(app_token(Some(""), Some("")));
        let publisher = connect_draft18_client(&relay).await;
        let subscriber = connect_draft18_client(&relay).await;

        // Act
        let published_forward = publish_track(&publisher, true).await;
        let subscription = subscribe_track(&subscriber, SubscribeOption::default()).await;
        within(subscriber.subscriber().unsubscribe(&subscription))
            .await
            .unwrap();
        let updated = is_sent_an_update(&publisher).await;

        // Assert
        assert!(published_forward);
        assert!(!updated);
    }
}
