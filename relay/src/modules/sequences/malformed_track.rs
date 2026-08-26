use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    relay::ingress::ingress_coordinator::IngressCommand,
    sequences::tables::table::{LocalPubSubDirectory, UpstreamSubscriptionKey},
    types::{SessionId, TrackKey},
};
use tracing::Span;

/// Upstream-side teardown after a §2.5 Malformed Track detection: the relay
/// is the subscriber toward the publisher, so it MUST UNSUBSCRIBE the
/// subscription for that track (in-flight upstream fetches send their own
/// FETCH_CANCEL from `FetchIngest`).
pub(crate) struct MalformedTrackCleanup;

impl MalformedTrackCleanup {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.malformed_track_cleanup",
        skip_all,
        parent = session_span,
        fields(publisher_session_id = %publisher_session_id, track_key = %track_key)
    )]
    pub(crate) async fn handle(
        &self,
        publisher_session_id: SessionId,
        session_span: &Span,
        track_key: &TrackKey,
        table: &dyn LocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
    ) {
        let upstream_key = UpstreamSubscriptionKey {
            publisher_session_id,
            track_namespace: track_key.track_namespace.clone(),
            track_name: track_key.track_name.clone(),
        };
        // Concurrent readers can report the same detection; only the first
        // report finds the subscription still registered.
        let Some(removed) = table.remove_upstream_subscription(&upstream_key) else {
            tracing::debug!("upstream subscription already removed");
            return;
        };

        if let Err(err) = forwarder
            .unsubscribe(publisher_session_id, removed.upstream_request_id)
            .await
        {
            tracing::warn!(
                ?err,
                request_id = %removed.upstream_request_id,
                "failed to send upstream unsubscribe for malformed track"
            );
        } else {
            tracing::info!(
                request_id = %removed.upstream_request_id,
                "sent upstream unsubscribe for malformed track"
            );
        }

        if ingress_sender
            .send(IngressCommand::StopTrack {
                track_key: track_key.clone(),
                publisher_session_id,
            })
            .await
            .is_err()
        {
            tracing::error!("failed to send ingress stop request");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::sync::mpsc;

    use super::*;
    use crate::modules::{
        core::{
            data_receiver::fetch_receiver::UpstreamFetchReceiver,
            data_receiver::receiver::DataReceiver, handler::publish::SubscribeOption,
            publisher::Publisher, session::Session, session_event::MoqtSessionEvent,
            subscriber::Subscriber, subscription::UpstreamSubscription,
        },
        enums::ContentExists,
        sequences::tables::{
            hashmap_table::InMemoryLocalPubSubDirectory,
            table::{ActiveUpstreamSubscription, UpstreamSubscriptionOrigin},
        },
        session_repository::SessionRepository,
    };

    const PUBLISHER_SESSION: SessionId = 1;
    const UPSTREAM_REQUEST_ID: u64 = 42;

    struct MockSubscriber {
        unsubscribed_request_ids: Arc<Mutex<Vec<u64>>>,
    }

    #[async_trait::async_trait]
    impl Subscriber for MockSubscriber {
        async fn send_subscribe(
            &mut self,
            _track_namespace: String,
            _track_name: String,
            _option: SubscribeOption,
        ) -> anyhow::Result<UpstreamSubscription> {
            unimplemented!("not used in malformed track cleanup tests")
        }

        async fn send_unsubscribe(&self, subscribe_id: u64) -> anyhow::Result<()> {
            self.unsubscribed_request_ids
                .lock()
                .unwrap()
                .push(subscribe_id);
            Ok(())
        }

        async fn send_unsubscribe_namespace(&self, _namespace: String) -> anyhow::Result<()> {
            unimplemented!("not used in malformed track cleanup tests")
        }

        async fn create_data_receiver(
            &mut self,
            _subscription: &UpstreamSubscription,
        ) -> anyhow::Result<DataReceiver> {
            unimplemented!("not used in malformed track cleanup tests")
        }

        async fn send_fetch(
            &mut self,
            _track_namespace: String,
            _track_name: String,
            _start_location: moqt::Location,
            _end_location: moqt::Location,
            _option: moqt::FetchOption,
        ) -> anyhow::Result<moqt::FetchHandle> {
            unimplemented!("not used in malformed track cleanup tests")
        }

        async fn create_fetch_receiver(
            &mut self,
            _handle: &moqt::FetchHandle,
        ) -> anyhow::Result<Box<dyn UpstreamFetchReceiver>> {
            unimplemented!("not used in malformed track cleanup tests")
        }

        async fn send_fetch_cancel(&self, _request_id: u64) -> anyhow::Result<()> {
            unimplemented!("not used in malformed track cleanup tests")
        }
    }

    struct MockSession {
        unsubscribed_request_ids: Arc<Mutex<Vec<u64>>>,
    }

    #[async_trait::async_trait]
    impl Session for MockSession {
        fn as_publisher(&self) -> Box<dyn Publisher> {
            unimplemented!("not used in malformed track cleanup tests")
        }

        fn as_subscriber(&self) -> Box<dyn Subscriber> {
            Box::new(MockSubscriber {
                unsubscribed_request_ids: self.unsubscribed_request_ids.clone(),
            })
        }

        async fn receive_moqt_session_event(&self) -> anyhow::Result<MoqtSessionEvent> {
            std::future::pending().await
        }
    }

    struct TestContext {
        table: InMemoryLocalPubSubDirectory,
        forwarder: ControlMessageForwarder,
        ingress_sender: mpsc::Sender<IngressCommand>,
        ingress_receiver: mpsc::Receiver<IngressCommand>,
        unsubscribed_request_ids: Arc<Mutex<Vec<u64>>>,
        track_key: TrackKey,
    }

    async fn setup() -> TestContext {
        let track_key = TrackKey::new("ns", "track");
        let table = InMemoryLocalPubSubDirectory::new();
        let upstream_key = UpstreamSubscriptionKey {
            publisher_session_id: PUBLISHER_SESSION,
            track_namespace: track_key.track_namespace.clone(),
            track_name: track_key.track_name.clone(),
        };
        table.register_upstream_subscription(
            upstream_key,
            ActiveUpstreamSubscription {
                upstream_request_id: UPSTREAM_REQUEST_ID,
                track_key: track_key.clone(),
                expires: None,
                content_exists: ContentExists::False,
                downstream_subscriber_count: 1,
                origin: UpstreamSubscriptionOrigin::Subscribe,
            },
        );

        let unsubscribed_request_ids = Arc::new(Mutex::new(Vec::new()));
        let mut repository = SessionRepository::new();
        let (session_event_sender, _session_event_receiver) = mpsc::unbounded_channel();
        repository
            .add_client(
                PUBLISHER_SESSION,
                Box::new(MockSession {
                    unsubscribed_request_ids: unsubscribed_request_ids.clone(),
                }),
                session_event_sender,
                tracing::Span::none(),
            )
            .await;
        let forwarder = ControlMessageForwarder {
            repository: Arc::new(tokio::sync::Mutex::new(repository)),
        };

        let (ingress_sender, ingress_receiver) = mpsc::channel(8);
        TestContext {
            table,
            forwarder,
            ingress_sender,
            ingress_receiver,
            unsubscribed_request_ids,
            track_key,
        }
    }

    async fn run_cleanup(ctx: &TestContext) {
        MalformedTrackCleanup
            .handle(
                PUBLISHER_SESSION,
                &tracing::Span::none(),
                &ctx.track_key,
                &ctx.table,
                &ctx.forwarder,
                &ctx.ingress_sender,
            )
            .await;
    }

    #[tokio::test]
    async fn detection_unsubscribes_upstream_and_stops_ingress() {
        let mut ctx = setup().await;

        // Act
        run_cleanup(&ctx).await;

        // Assert: the upstream subscription is unregistered and unsubscribed.
        assert!(
            ctx.table
                .get_active_upstream_subscription(PUBLISHER_SESSION, "ns", "track")
                .is_none()
        );
        assert_eq!(
            *ctx.unsubscribed_request_ids.lock().unwrap(),
            vec![UPSTREAM_REQUEST_ID]
        );
        // Assert: ingress is told to stop the track.
        match ctx.ingress_receiver.try_recv() {
            Ok(IngressCommand::StopTrack {
                track_key,
                publisher_session_id,
            }) => {
                assert_eq!(track_key, ctx.track_key);
                assert_eq!(publisher_session_id, PUBLISHER_SESSION);
            }
            other => panic!("Expected StopTrack, got {:?}", other.is_ok()),
        }
    }

    #[tokio::test]
    async fn duplicate_detection_reports_are_idempotent() {
        let mut ctx = setup().await;
        run_cleanup(&ctx).await;
        let _ = ctx.ingress_receiver.try_recv();

        // Act: a second reader reports the same detection.
        run_cleanup(&ctx).await;

        // Assert: no second unsubscribe or stop is issued.
        assert_eq!(
            *ctx.unsubscribed_request_ids.lock().unwrap(),
            vec![UPSTREAM_REQUEST_ID]
        );
        assert!(ctx.ingress_receiver.try_recv().is_err());
    }
}
