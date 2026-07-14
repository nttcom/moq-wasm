use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    core::handler::unsubscribe::UnsubscribeHandler,
    relay::{egress::coordinator::EgressCommand, ingress::ingress_coordinator::IngressCommand},
    sequences::tables::table::{LocalPubSubDirectory, UpstreamSubscriptionOrigin},
    types::SessionId,
};
use tracing::Span;

pub(crate) struct Unsubscribe;

impl Unsubscribe {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.unsubscribe",
        skip_all,
        parent = session_span,
        fields(session_id = %session_id)
    )]
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &dyn LocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        handler: Box<dyn UnsubscribeHandler>,
    ) {
        let subscribe_id = handler.subscribe_id();
        tracing::info!(
            session_id = %session_id,
            subscribe_id = %subscribe_id,
            "SequenceHandler::unsubscribe"
        );

        let Some(removed) = table.remove_downstream_subscription(session_id, subscribe_id) else {
            tracing::warn!(
                session_id = %session_id,
                subscribe_id = %subscribe_id,
                "active downstream subscription not found"
            );
            return;
        };

        if egress_sender
            .send(EgressCommand::StopReader {
                subscriber_session_id: session_id,
                downstream_subscribe_id: subscribe_id,
            })
            .await
            .is_err()
        {
            tracing::error!("Failed to send EgressStopRequest.");
        }

        tracing::info!(
            session_id = %session_id,
            subscribe_id = %subscribe_id,
            upstream_session_id = %removed.upstream_key.publisher_session_id,
            track_namespace = %removed.upstream_key.track_namespace,
            track_name = %removed.upstream_key.track_name,
            remaining_downstream_subscriber_count = removed.remaining_downstream_subscriber_count,
            "downstream unsubscribe processed"
        );

        if removed.remaining_downstream_subscriber_count == 0
            && removed.upstream_origin == UpstreamSubscriptionOrigin::Subscribe
        {
            if let Err(err) = forwarder
                .unsubscribe(
                    removed.upstream_key.publisher_session_id,
                    removed.upstream_request_id,
                )
                .await
            {
                tracing::warn!(
                    ?err,
                    upstream_session_id = %removed.upstream_key.publisher_session_id,
                    request_id = %removed.upstream_request_id,
                    "failed to forward upstream unsubscribe"
                );
            } else {
                tracing::info!(
                    upstream_session_id = %removed.upstream_key.publisher_session_id,
                    request_id = %removed.upstream_request_id,
                    "forwarded upstream unsubscribe"
                );
            }

            if ingress_sender
                .send(IngressCommand::StopTrack {
                    track_key: removed.track_key.clone(),
                    publisher_session_id: removed.upstream_key.publisher_session_id,
                })
                .await
                .is_err()
            {
                tracing::error!(
                    track_key = %removed.track_key,
                    "failed to send ingress stop request"
                );
            }
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
            data_receiver::receiver::DataReceiver, handler::publish::SubscribeOption,
            publisher::Publisher, session::Session, session_event::MoqtSessionEvent,
            subscriber::Subscriber, subscription::UpstreamSubscription,
        },
        enums::ContentExists,
        sequences::tables::{
            hashmap_table::InMemoryLocalPubSubDirectory,
            table::{ActiveUpstreamSubscription, UpstreamSubscriptionKey},
        },
        session_repository::SessionRepository,
        types::TrackKey,
    };

    const PUBLISHER_SESSION: SessionId = 1;
    const UPSTREAM_REQUEST_ID: u64 = 42;

    struct MockUnsubscribeHandler {
        subscribe_id: u64,
    }

    impl UnsubscribeHandler for MockUnsubscribeHandler {
        fn subscribe_id(&self) -> u64 {
            self.subscribe_id
        }
    }

    struct MockSubscriber {
        unsubscribed_request_ids: Arc<Mutex<Vec<u64>>>,
    }

    #[async_trait::async_trait]
    impl Subscriber for MockSubscriber {
        async fn _send_subscribe_namespace(&self, _namespace: String) -> anyhow::Result<()> {
            Ok(())
        }

        async fn send_subscribe(
            &mut self,
            _track_namespace: String,
            _track_name: String,
            _option: SubscribeOption,
        ) -> anyhow::Result<UpstreamSubscription> {
            unimplemented!("not used in unsubscribe tests")
        }

        async fn send_unsubscribe(&self, subscribe_id: u64) -> anyhow::Result<()> {
            self.unsubscribed_request_ids
                .lock()
                .unwrap()
                .push(subscribe_id);
            Ok(())
        }

        async fn send_unsubscribe_namespace(&self, _namespace: String) -> anyhow::Result<()> {
            Ok(())
        }

        async fn create_data_receiver(
            &mut self,
            _subscription: &UpstreamSubscription,
        ) -> anyhow::Result<DataReceiver> {
            unimplemented!("not used in unsubscribe tests")
        }
    }

    struct MockSession {
        unsubscribed_request_ids: Arc<Mutex<Vec<u64>>>,
    }

    #[async_trait::async_trait]
    impl Session for MockSession {
        fn as_publisher(&self) -> Box<dyn Publisher> {
            unimplemented!("not used in unsubscribe tests")
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
        egress_sender: mpsc::Sender<EgressCommand>,
        egress_receiver: mpsc::Receiver<EgressCommand>,
        unsubscribed_request_ids: Arc<Mutex<Vec<u64>>>,
    }

    async fn setup(
        origin: UpstreamSubscriptionOrigin,
        downstream_subscriptions: &[(SessionId, u64)],
    ) -> TestContext {
        let table = InMemoryLocalPubSubDirectory::new();
        let upstream_key = UpstreamSubscriptionKey {
            publisher_session_id: PUBLISHER_SESSION,
            track_namespace: "ns".to_string(),
            track_name: "track".to_string(),
        };
        table.register_upstream_subscription(
            upstream_key.clone(),
            ActiveUpstreamSubscription {
                upstream_request_id: UPSTREAM_REQUEST_ID,
                track_key: TrackKey::new("ns", "track"),
                expires: None,
                content_exists: ContentExists::False,
                downstream_subscriber_count: 0,
                origin,
            },
        );
        for (session_id, subscribe_id) in downstream_subscriptions {
            assert!(table.register_downstream_subscription(
                *session_id,
                *subscribe_id,
                upstream_key.clone(),
                None,
            ));
        }

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
        let (egress_sender, egress_receiver) = mpsc::channel(8);
        TestContext {
            table,
            forwarder,
            ingress_sender,
            ingress_receiver,
            egress_sender,
            egress_receiver,
            unsubscribed_request_ids,
        }
    }

    async fn run_unsubscribe(ctx: &TestContext, session_id: SessionId, subscribe_id: u64) {
        Unsubscribe
            .handle(
                session_id,
                &tracing::Span::none(),
                &ctx.table,
                &ctx.forwarder,
                &ctx.ingress_sender,
                &ctx.egress_sender,
                Box::new(MockUnsubscribeHandler { subscribe_id }),
            )
            .await;
    }

    #[tokio::test]
    async fn last_subscriber_forwards_upstream_unsubscribe_and_stops_ingress() {
        let mut ctx = setup(UpstreamSubscriptionOrigin::Subscribe, &[(100, 10)]).await;

        run_unsubscribe(&ctx, 100, 10).await;

        match ctx.egress_receiver.try_recv() {
            Ok(EgressCommand::StopReader {
                subscriber_session_id,
                downstream_subscribe_id,
            }) => {
                assert_eq!(subscriber_session_id, 100);
                assert_eq!(downstream_subscribe_id, 10);
            }
            other => panic!("Expected StopReader, got {:?}", other.is_ok()),
        }
        assert_eq!(
            *ctx.unsubscribed_request_ids.lock().unwrap(),
            vec![UPSTREAM_REQUEST_ID]
        );
        match ctx.ingress_receiver.try_recv() {
            Ok(IngressCommand::StopTrack {
                track_key,
                publisher_session_id,
            }) => {
                assert_eq!(track_key, TrackKey::new("ns", "track"));
                assert_eq!(publisher_session_id, PUBLISHER_SESSION);
            }
            other => panic!("Expected StopTrack, got {:?}", other.is_ok()),
        }
    }

    #[tokio::test]
    async fn remaining_subscribers_keep_upstream_subscription() {
        let mut ctx = setup(
            UpstreamSubscriptionOrigin::Subscribe,
            &[(100, 10), (101, 11)],
        )
        .await;

        run_unsubscribe(&ctx, 100, 10).await;

        assert!(matches!(
            ctx.egress_receiver.try_recv(),
            Ok(EgressCommand::StopReader { .. })
        ));
        assert!(ctx.unsubscribed_request_ids.lock().unwrap().is_empty());
        assert!(ctx.ingress_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn publish_origin_keeps_upstream_subscription() {
        let mut ctx = setup(UpstreamSubscriptionOrigin::Publish, &[(100, 10)]).await;

        run_unsubscribe(&ctx, 100, 10).await;

        assert!(matches!(
            ctx.egress_receiver.try_recv(),
            Ok(EgressCommand::StopReader { .. })
        ));
        assert!(ctx.unsubscribed_request_ids.lock().unwrap().is_empty());
        assert!(ctx.ingress_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn unknown_subscription_sends_no_commands() {
        let mut ctx = setup(UpstreamSubscriptionOrigin::Subscribe, &[]).await;

        run_unsubscribe(&ctx, 100, 10).await;

        assert!(ctx.egress_receiver.try_recv().is_err());
        assert!(ctx.unsubscribed_request_ids.lock().unwrap().is_empty());
        assert!(ctx.ingress_receiver.try_recv().is_err());
    }
}
