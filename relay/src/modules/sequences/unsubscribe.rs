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
    use tokio::sync::mpsc;

    use super::*;
    use crate::modules::{
        core::mocks::{RecordedControlMessages, session_repository_with_upstream_session},
        enums::ContentExists,
        sequences::tables::{
            hashmap_table::InMemoryLocalPubSubDirectory,
            table::{ActiveUpstreamSubscription, UpstreamSubscriptionKey},
        },
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

    struct TestContext {
        table: InMemoryLocalPubSubDirectory,
        forwarder: ControlMessageForwarder,
        ingress_sender: mpsc::Sender<IngressCommand>,
        ingress_receiver: mpsc::Receiver<IngressCommand>,
        egress_sender: mpsc::Sender<EgressCommand>,
        egress_receiver: mpsc::Receiver<EgressCommand>,
        recorded: RecordedControlMessages,
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

        let (repository, recorded) =
            session_repository_with_upstream_session(PUBLISHER_SESSION).await;
        let forwarder = ControlMessageForwarder { repository };

        let (ingress_sender, ingress_receiver) = mpsc::channel(8);
        let (egress_sender, egress_receiver) = mpsc::channel(8);
        TestContext {
            table,
            forwarder,
            ingress_sender,
            ingress_receiver,
            egress_sender,
            egress_receiver,
            recorded,
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
        // Arrange
        let mut ctx = setup(UpstreamSubscriptionOrigin::Subscribe, &[(100, 10)]).await;

        // Act
        run_unsubscribe(&ctx, 100, 10).await;

        // Assert
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
            *ctx.recorded.unsubscribed_request_ids.lock().unwrap(),
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
        // Arrange
        let mut ctx = setup(
            UpstreamSubscriptionOrigin::Subscribe,
            &[(100, 10), (101, 11)],
        )
        .await;

        // Act
        run_unsubscribe(&ctx, 100, 10).await;

        // Assert
        assert!(matches!(
            ctx.egress_receiver.try_recv(),
            Ok(EgressCommand::StopReader { .. })
        ));
        assert!(
            ctx.recorded
                .unsubscribed_request_ids
                .lock()
                .unwrap()
                .is_empty()
        );
        assert!(ctx.ingress_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn publish_origin_keeps_upstream_subscription() {
        // Arrange
        let mut ctx = setup(UpstreamSubscriptionOrigin::Publish, &[(100, 10)]).await;

        // Act
        run_unsubscribe(&ctx, 100, 10).await;

        // Assert
        assert!(matches!(
            ctx.egress_receiver.try_recv(),
            Ok(EgressCommand::StopReader { .. })
        ));
        assert!(
            ctx.recorded
                .unsubscribed_request_ids
                .lock()
                .unwrap()
                .is_empty()
        );
        assert!(ctx.ingress_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn unknown_subscription_sends_no_commands() {
        // Arrange
        let mut ctx = setup(UpstreamSubscriptionOrigin::Subscribe, &[]).await;

        // Act
        run_unsubscribe(&ctx, 100, 10).await;

        // Assert
        assert!(ctx.egress_receiver.try_recv().is_err());
        assert!(
            ctx.recorded
                .unsubscribed_request_ids
                .lock()
                .unwrap()
                .is_empty()
        );
        assert!(ctx.ingress_receiver.try_recv().is_err());
    }
}
