mod publish_namespace_response_task;

use publish_namespace_response_task::PublishNamespaceResponseTask;

use std::sync::Arc;

use moqt::{
    TrackProperties,
    object::{ContentExists, GroupOrder},
    subscribe::{FilterType, SubscribeOption},
};

use crate::modules::{
    domain::{publish_done_reason::PublishDoneReason, session_id::SessionId},
    session::{
        publisher::Publisher,
        session_repository::SessionRepository,
        subscriber::Subscriber,
        subscription::{DownstreamSubscription, UpstreamSubscription},
    },
};

#[derive(Clone)]
pub(crate) struct ControlMessageForwarder {
    pub(crate) repository: Arc<tokio::sync::Mutex<SessionRepository>>,
}

impl ControlMessageForwarder {
    async fn publisher(&self, session_id: SessionId) -> anyhow::Result<Box<dyn Publisher>> {
        let publisher = self.repository.lock().await.publisher(session_id);
        publisher.ok_or_else(|| anyhow::anyhow!("No publisher"))
    }

    async fn subscriber(&self, session_id: SessionId) -> anyhow::Result<Box<dyn Subscriber>> {
        let subscriber = self.repository.lock().await.subscriber(session_id);
        subscriber.ok_or_else(|| anyhow::anyhow!("No subscriber"))
    }

    /// Records and sends PUBLISH_NAMESPACE on the session while `wanted`
    /// holds, checked under the session's announcement lock just before the
    /// record, so the decision and the send are not overtaken by another
    /// announcement or withdrawal of the namespace on the session.
    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.publish_namespace",
        skip_all,
        fields(subscriber_session_id = %session_id, track_namespace = %track_namespace)
    )]
    pub(crate) async fn publish_namespace(
        &self,
        session_id: SessionId,
        track_namespace: String,
        wanted: impl FnOnce() -> bool,
    ) {
        let Some(announcement_lock) = self.repository.lock().await.announcement_lock(session_id)
        else {
            tracing::warn!(subscriber_session_id = %session_id, %track_namespace, "PUBLISH_NAMESPACE not forwarded: no publisher");
            return;
        };
        let _announcing = announcement_lock.lock().await;
        let publisher = {
            let mut repository = self.repository.lock().await;
            let Some(publisher) = repository.publisher(session_id) else {
                tracing::warn!(subscriber_session_id = %session_id, %track_namespace, "PUBLISH_NAMESPACE not forwarded: no publisher");
                return;
            };
            if !wanted() {
                tracing::debug!("namespace no longer to be announced on the session");
                return;
            }
            if !repository.record_namespace_announced(session_id, &track_namespace) {
                tracing::debug!("namespace already announced on the session");
                return;
            }
            publisher
        };
        match publisher.send_publish_namespace(track_namespace).await {
            Ok(response) => {
                let _response_task = PublishNamespaceResponseTask::run(response);
                tracing::debug!("forwarded PUBLISH_NAMESPACE");
            }
            Err(error) => tracing::error!(?error, "Failed to send publish namespace"),
        }
    }

    pub(crate) async fn forget_announced_namespaces(
        &self,
        session_id: SessionId,
        track_namespace_prefix: &str,
        kept_prefix: Option<&str>,
    ) {
        self.repository.lock().await.forget_announced_namespaces(
            session_id,
            track_namespace_prefix,
            kept_prefix,
        );
    }

    /// `publish_namespace` for PUBLISH_NAMESPACE_DONE; returns whether it was
    /// sent.
    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.publish_namespace_done",
        skip_all,
        fields(subscriber_session_id = %session_id, track_namespace = %track_namespace)
    )]
    pub(crate) async fn publish_namespace_done(
        &self,
        session_id: SessionId,
        track_namespace: String,
        wanted: impl FnOnce() -> bool,
    ) -> bool {
        let Some(announcement_lock) = self.repository.lock().await.announcement_lock(session_id)
        else {
            tracing::warn!(subscriber_session_id = %session_id, %track_namespace, "PUBLISH_NAMESPACE_DONE not forwarded: no publisher");
            return false;
        };
        let _announcing = announcement_lock.lock().await;
        let publisher = {
            let mut repository = self.repository.lock().await;
            let Some(publisher) = repository.publisher(session_id) else {
                tracing::warn!(subscriber_session_id = %session_id, %track_namespace, "PUBLISH_NAMESPACE_DONE not forwarded: no publisher");
                return false;
            };
            if !wanted() {
                tracing::debug!("namespace no longer to be withdrawn on the session");
                return false;
            }
            let announced = repository.record_namespace_withdrawn(session_id, &track_namespace);
            // A relay learns a namespace published on another relay from the
            // route registry rather than from a PUBLISH_NAMESPACE on this
            // session, and accepts a DONE for one it never registered.
            if !announced && repository.is_client_session(session_id) {
                tracing::debug!("namespace not announced on the session");
                return false;
            }
            publisher
        };
        match publisher.send_publish_namespace_done(track_namespace).await {
            Ok(()) => tracing::debug!("forwarded PUBLISH_NAMESPACE_DONE"),
            Err(error) => tracing::error!(?error, "Failed to send publish namespace done"),
        }
        true
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.publish",
        skip_all,
        fields(subscriber_session_id = %session_id, track_namespace = %track_namespace, track_name = %track_name)
    )]
    pub(crate) async fn publish(
        &self,
        session_id: SessionId,
        track_namespace: String,
        track_name: String,
        content_exists: ContentExists,
        track_properties: TrackProperties,
        forward: bool,
    ) -> anyhow::Result<DownstreamSubscription> {
        let publisher = self.publisher(session_id).await?;
        publisher
            .send_publish(
                track_namespace,
                track_name,
                content_exists,
                track_properties,
                forward,
            )
            .await
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.publish_done",
        skip_all,
        fields(subscriber_session_id = %session_id, request_id = %subscription.request_id())
    )]
    pub(crate) async fn publish_done(
        &self,
        session_id: SessionId,
        subscription: &DownstreamSubscription,
        reason: PublishDoneReason,
    ) -> anyhow::Result<()> {
        let publisher = self.publisher(session_id).await?;
        publisher
            .send_publish_done(subscription, reason.status_code, 0, reason.error_reason)
            .await
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.subscribe",
        skip_all,
        fields(publisher_session_id = %session_id, track_namespace = %track_namespace, track_name = %track_name)
    )]
    pub(crate) async fn subscribe(
        &self,
        session_id: SessionId,
        track_namespace: String,
        track_name: String,
        new_group_request: Option<u64>,
    ) -> anyhow::Result<UpstreamSubscription> {
        let mut subscriber = self.subscriber(session_id).await?;
        let option = SubscribeOption {
            subscriber_priority: 128,
            group_order: GroupOrder::Ascending,
            forward: true,
            filter_type: FilterType::LargestObject,
            new_group_request,
            delivery_timeout: None,
        };
        tracing::debug!("forwarding SUBSCRIBE");
        subscriber
            .send_subscribe(track_namespace, track_name, option)
            .await
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.new_group_request",
        skip_all,
        fields(publisher_session_id = %session_id, request_id = %subscription.request_id())
    )]
    pub(crate) async fn new_group_request(
        &self,
        session_id: SessionId,
        subscription: &UpstreamSubscription,
        new_group_request: u64,
    ) -> anyhow::Result<bool> {
        let subscriber = self.subscriber(session_id).await?;
        tracing::debug!("forwarding NEW_GROUP_REQUEST");
        subscriber
            .send_new_group_request(subscription, new_group_request)
            .await
    }

    pub(crate) async fn update_forward(
        &self,
        session_id: SessionId,
        subscription: &UpstreamSubscription,
        forward: bool,
    ) -> anyhow::Result<()> {
        let subscriber = self.subscriber(session_id).await?;
        subscriber.send_forward_update(subscription, forward).await
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.fetch",
        skip_all,
        fields(publisher_session_id = %session_id, track_namespace = %track_namespace, track_name = %track_name)
    )]
    pub(crate) async fn fetch(
        &self,
        session_id: SessionId,
        track_namespace: String,
        track_name: String,
        start_location: moqt::object::Location,
        end_location: moqt::object::Location,
        option: moqt::fetch::FetchOption,
    ) -> anyhow::Result<moqt::fetch::FetchHandle> {
        let mut subscriber = self.subscriber(session_id).await?;
        tracing::debug!("forwarding FETCH");
        let handle = subscriber
            .send_fetch(
                track_namespace,
                track_name,
                start_location,
                end_location,
                option,
            )
            .await?;
        Ok(handle)
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.unsubscribe",
        skip_all,
        fields(publisher_session_id = %session_id, request_id = %subscription.request_id())
    )]
    pub(crate) async fn unsubscribe(
        &self,
        session_id: SessionId,
        subscription: &UpstreamSubscription,
    ) -> anyhow::Result<()> {
        let subscriber = self.subscriber(session_id).await?;
        tracing::debug!("forwarding UNSUBSCRIBE");
        subscriber.send_unsubscribe(subscription).await
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.subscribe_namespace",
        skip_all,
        fields(publisher_session_id = %session_id, track_namespace_prefix = %track_namespace_prefix)
    )]
    pub(crate) async fn subscribe_namespace(
        &self,
        session_id: SessionId,
        track_namespace_prefix: String,
    ) -> anyhow::Result<()> {
        let subscriber = self.subscriber(session_id).await?;
        subscriber
            .send_subscribe_namespace(track_namespace_prefix)
            .await
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.unsubscribe_namespace",
        skip_all,
        fields(publisher_session_id = %session_id, track_namespace_prefix = %track_namespace_prefix)
    )]
    pub(crate) async fn unsubscribe_namespace(
        &self,
        session_id: SessionId,
        track_namespace_prefix: String,
    ) -> anyhow::Result<()> {
        let subscriber = self.subscriber(session_id).await?;
        tracing::debug!("forwarding UNSUBSCRIBE_NAMESPACE");
        subscriber
            .send_unsubscribe_namespace(track_namespace_prefix)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{
        auth::verified_token::VerifiedToken,
        domain::session_peer::SessionPeer,
        session::Session,
        session::session_repository::NewSession,
        test_support::mock_session::{
            RecordedControlMessages, recorded_session_answering_subscribe,
            recorded_session_delaying_publish_namespace, session_repository,
        },
    };

    const NAMESPACE: &str = "room/alice";
    const SUBSCRIBER: SessionId = 1;

    fn forwarder_with_subscriber(
        peer: SessionPeer,
    ) -> (ControlMessageForwarder, RecordedControlMessages) {
        let (session, recorded) = recorded_session_answering_subscribe(|| ContentExists::False);
        (forwarder_with_session(peer, session), recorded)
    }

    fn forwarder_with_session(
        peer: SessionPeer,
        session: Box<dyn Session>,
    ) -> ControlMessageForwarder {
        let (mut repository, _session_added_receiver) = session_repository();
        repository.add(NewSession {
            session_id: SUBSCRIBER,
            session,
            session_span: tracing::Span::none(),
            peer,
            verified_token: VerifiedToken::full_access(),
        });
        ControlMessageForwarder {
            repository: Arc::new(tokio::sync::Mutex::new(repository)),
        }
    }

    async fn let_tasks_run() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn a_relay_is_told_done_for_a_namespace_it_learned_from_the_route_registry() {
        // Arrange
        let (forwarder, recorded) = forwarder_with_subscriber(SessionPeer::Relay);

        // Act
        forwarder
            .publish_namespace_done(SUBSCRIBER, NAMESPACE.to_string(), || true)
            .await;

        // Assert
        assert_eq!(
            recorded.publish_namespace_dones(),
            vec![NAMESPACE.to_string()]
        );
    }

    #[tokio::test]
    async fn a_client_is_not_told_done_for_a_namespace_never_announced_to_it() {
        // Arrange
        let (forwarder, recorded) = forwarder_with_subscriber(SessionPeer::Client);

        // Act
        forwarder
            .publish_namespace_done(SUBSCRIBER, NAMESPACE.to_string(), || true)
            .await;

        // Assert
        assert!(recorded.publish_namespace_dones().is_empty());
    }

    #[tokio::test]
    async fn a_client_is_told_done_for_a_namespace_announced_to_it() {
        // Arrange
        let (forwarder, recorded) = forwarder_with_subscriber(SessionPeer::Client);
        forwarder
            .publish_namespace(SUBSCRIBER, NAMESPACE.to_string(), || true)
            .await;

        // Act
        forwarder
            .publish_namespace_done(SUBSCRIBER, NAMESPACE.to_string(), || true)
            .await;

        // Assert
        assert_eq!(
            recorded.publish_namespace_dones(),
            vec![NAMESPACE.to_string()]
        );
    }

    #[tokio::test]
    async fn a_withdrawal_crossing_an_announcement_reaches_the_subscriber_after_it() {
        // Arrange: the PUBLISH_NAMESPACE is on its way to the subscriber
        let delay = Arc::new(tokio::sync::Semaphore::new(0));
        let (session, recorded) = recorded_session_delaying_publish_namespace(delay.clone());
        let forwarder = forwarder_with_session(SessionPeer::Client, session);
        let announcement = tokio::spawn({
            let forwarder = forwarder.clone();
            async move {
                forwarder
                    .publish_namespace(SUBSCRIBER, NAMESPACE.to_string(), || true)
                    .await;
            }
        });
        let_tasks_run().await;

        // Act
        let withdrawal = tokio::spawn({
            let forwarder = forwarder.clone();
            async move {
                forwarder
                    .publish_namespace_done(SUBSCRIBER, NAMESPACE.to_string(), || true)
                    .await;
            }
        });
        let_tasks_run().await;
        delay.add_permits(1);
        announcement.await.unwrap();
        withdrawal.await.unwrap();

        // Assert
        assert_eq!(
            recorded.namespace_announcements(),
            vec![format!("publish {NAMESPACE}"), format!("done {NAMESPACE}")]
        );
    }

    #[tokio::test]
    async fn a_withdrawal_no_longer_wanted_keeps_the_namespace_announced() {
        // Arrange
        let (forwarder, recorded) = forwarder_with_subscriber(SessionPeer::Client);
        forwarder
            .publish_namespace(SUBSCRIBER, NAMESPACE.to_string(), || true)
            .await;

        // Act
        let sent = forwarder
            .publish_namespace_done(SUBSCRIBER, NAMESPACE.to_string(), || false)
            .await;
        forwarder
            .publish_namespace(SUBSCRIBER, NAMESPACE.to_string(), || true)
            .await;

        // Assert: no PUBLISH_NAMESPACE_DONE, and the namespace is not announced twice
        assert!(!sent);
        assert!(recorded.publish_namespace_dones().is_empty());
        assert_eq!(recorded.publish_namespaces(), vec![NAMESPACE.to_string()]);
    }
}
