use std::{sync::Weak, time::Duration};

use futures::{StreamExt, stream::FuturesUnordered};
use tokio::sync::mpsc;
use tracing::{Instrument, Span};

use crate::modules::{
    executor::{self, JoinHandle},
    moqt::{
        control_plane::handler::publish_done_handler::PublishDoneHandler,
        domains::{connection::Connection, session_context::SessionContext},
    },
};

/// draft-14 §9.12: PUBLISH_DONE travels on the control stream and can arrive
/// before the subscription's last data streams. When they do not all arrive (a
/// Stream Count of 2^62-1, a stream reset before its header), or the Stream
/// Count is 0, the subscription ends this period after PUBLISH_DONE.
pub(crate) const PUBLISH_DONE_GRACE_PERIOD: Duration = Duration::from_secs(10);

/// Cancels a subscription once the streams its PUBLISH_DONE counts have
/// arrived, or `PUBLISH_DONE_GRACE_PERIOD` after it, like an unsubscribe but
/// without sending UNSUBSCRIBE. Its `TrackReader` then returns `Ok(None)` once
/// the streams it already accepted are drained.
pub(crate) struct PublishDoneExpiryTask;

impl PublishDoneExpiryTask {
    pub(crate) fn run(
        mut publish_done_receiver: mpsc::UnboundedReceiver<PublishDoneHandler>,
        session_context: Weak<SessionContext>,
        connection: Weak<Connection>,
        session_span: Span,
    ) -> JoinHandle {
        executor::spawn(
            "Publish Done Expiry",
            async move {
                let mut expiries = FuturesUnordered::new();
                loop {
                    tokio::select! {
                        Some(publish_done) = publish_done_receiver.recv() => {
                            let request_id = publish_done.request_id();
                            let arrived_streams = match connection.upgrade() {
                                // A datagram track sends 0, and its datagrams can still
                                // follow PUBLISH_DONE, so 0 waits the grace period.
                                Some(connection) if publish_done.stream_count() > 0 => {
                                    connection.arrived_streams(request_id).await
                                }
                                _ => None,
                            };
                            expiries.push(async move {
                                let _ = executor::timeout(PUBLISH_DONE_GRACE_PERIOD, async {
                                    match arrived_streams {
                                        Some(mut arrived_streams) => {
                                            // Fails once the sink is gone, which ends the wait too.
                                            let _ = arrived_streams
                                                .wait_for(|count| *count >= publish_done.stream_count())
                                                .await;
                                        }
                                        None => std::future::pending().await,
                                    }
                                })
                                .await;
                                request_id
                            });
                        }
                        Some(request_id) = expiries.next() => {
                            let (Some(session), Some(connection)) =
                                (session_context.upgrade(), connection.upgrade())
                            else {
                                break;
                            };
                            tracing::debug!(request_id, "ending the subscription PUBLISH_DONE terminated");
                            session.cancel_subscription(&connection, request_id).await;
                        }
                        else => break,
                    }
                }
            }
            .instrument(session_span),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::{sync::mpsc::error::TryRecvError, time::Instant};

    use super::PUBLISH_DONE_GRACE_PERIOD;
    use crate::{
        Session, SessionEvent, Subscription, Version,
        modules::{
            moqt::{
                control_plane::error_codes::PublishDoneCode,
                runtime::dispatch::incoming_object::{IncomingObject, IncomingObjectReceiver},
            },
            test_support::{
                DRAFTS, connected_sessions, open_subgroup, send_payload, take_data_receiver, within,
            },
        },
        object::ContentExists,
        subscribe::SubscribeOption,
    };

    /// A subscription the server made to the client's `ns/track`.
    struct ServedSubscription {
        client: Session,
        server: Session,
        served: Subscription,
        object_receiver: IncomingObjectReceiver,
    }

    async fn served_subscription(name: &str, version: Version) -> ServedSubscription {
        let (client, server) = connected_sessions(name, version).await;
        let mut subscriber = server.subscriber();
        let (subscribed, served) = within(async {
            tokio::join!(
                subscriber.subscribe("ns".into(), "track".into(), SubscribeOption::default()),
                async {
                    let SessionEvent::Subscribe(handler) = client.receive_event().await.unwrap()
                    else {
                        panic!("expected SUBSCRIBE from the server");
                    };
                    let track_alias = handler.ok(0, ContentExists::False).await.unwrap();
                    handler.into_subscription(track_alias)
                }
            )
        })
        .await;
        let object_receiver =
            take_data_receiver(&subscriber.session, subscribed.unwrap().track_alias()).await;
        ServedSubscription {
            client,
            server,
            served,
            object_receiver,
        }
    }

    /// Returns once the server has received the PUBLISH_DONE.
    async fn end_with_publish_done(subscription: &ServedSubscription, stream_count: u64) {
        subscription
            .client
            .publisher()
            .publish_done(
                &subscription.served,
                PublishDoneCode::TrackEnded,
                stream_count,
                String::new(),
            )
            .await
            .unwrap();
        let SessionEvent::PublishDone(_) =
            within(subscription.server.receive_event()).await.unwrap()
        else {
            panic!("expected PUBLISH_DONE from the client");
        };
    }

    #[tokio::test]
    async fn subscription_ends_once_the_streams_publish_done_counts_have_arrived() {
        for version in DRAFTS {
            // Arrange
            let mut subscription =
                served_subscription("publish-done-streams-arrived", version).await;
            let factory = subscription
                .client
                .publisher()
                .create_stream(&subscription.served);
            let mut group = open_subgroup(&factory, 0).await;
            send_payload(&mut group, b"object").await;
            let arrived = within(subscription.object_receiver.recv()).await;
            assert!(matches!(arrived, Some(IncomingObject::Subgroup(_))));

            // Act
            end_with_publish_done(&subscription, 1).await;
            let ended =
                tokio::time::timeout(Duration::from_secs(1), subscription.object_receiver.recv())
                    .await;

            // Assert: it ends well before the grace period
            assert!(matches!(ended, Ok(None)), "{version:?}");
        }
    }

    #[tokio::test]
    async fn subscription_stays_open_while_a_counted_stream_has_not_arrived() {
        for version in DRAFTS {
            // Arrange
            let mut subscription = served_subscription("publish-done-grace", version).await;
            let publish_done_sent_at = Instant::now();
            end_with_publish_done(&subscription, 1).await;
            tokio::time::pause();

            // Act
            tokio::time::sleep_until(
                publish_done_sent_at + PUBLISH_DONE_GRACE_PERIOD - Duration::from_millis(1),
            )
            .await;

            // Assert
            assert!(matches!(
                subscription.object_receiver.try_recv(),
                Err(TryRecvError::Empty)
            ));
            tokio::time::resume();
        }
    }

    #[tokio::test]
    async fn subscription_with_a_stream_count_of_0_stays_open_for_the_grace_period() {
        for version in DRAFTS {
            // Arrange: a datagram track ends with Stream Count 0
            let mut subscription = served_subscription("publish-done-zero-streams", version).await;
            let publish_done_sent_at = Instant::now();
            end_with_publish_done(&subscription, 0).await;
            tokio::time::pause();

            // Act
            tokio::time::sleep_until(
                publish_done_sent_at + PUBLISH_DONE_GRACE_PERIOD - Duration::from_millis(1),
            )
            .await;

            // Assert
            assert!(matches!(
                subscription.object_receiver.try_recv(),
                Err(TryRecvError::Empty)
            ));
            tokio::time::resume();
        }
    }

    #[tokio::test]
    async fn subscription_ends_after_the_grace_period_without_its_counted_streams() {
        for version in DRAFTS {
            // Arrange
            let mut subscription = served_subscription("publish-done-expiry", version).await;
            end_with_publish_done(&subscription, 1).await;
            tokio::time::pause();

            // Act
            tokio::time::sleep(PUBLISH_DONE_GRACE_PERIOD).await;

            // Assert
            let received =
                tokio::time::timeout(Duration::from_secs(1), subscription.object_receiver.recv())
                    .await;
            assert!(matches!(received, Ok(None)));
            tokio::time::resume();
        }
    }
}
