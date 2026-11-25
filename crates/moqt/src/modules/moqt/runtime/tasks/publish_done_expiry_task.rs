use std::{
    collections::HashSet,
    sync::{Arc, Weak},
    time::Duration,
};

use futures::{StreamExt, stream::FuturesUnordered};
use tokio::sync::{mpsc, watch};
use tracing::{Instrument, Span};

use crate::modules::{
    executor::{self, JoinHandle},
    moqt::{
        control_plane::{enums::RequestId, handler::publish_done_handler::PublishDoneHandler},
        domains::{connection::Connection, session_context::SessionContext},
    },
};

/// draft-14 §9.12: PUBLISH_DONE travels on the control stream and can arrive
/// before the subscription's last data streams. When they do not all arrive (a
/// Stream Count of 2^62-1, a stream reset before its header), or the Stream
/// Count is 0, the subscription ends this period after PUBLISH_DONE.
pub(crate) const PUBLISH_DONE_GRACE_PERIOD: Duration = Duration::from_secs(10);

/// Ends a subscription once the streams its PUBLISH_DONE counts have
/// arrived, or `PUBLISH_DONE_GRACE_PERIOD` after it, without sending
/// UNSUBSCRIBE. Its `TrackReader` then returns `Ok(None)` once
/// the streams it already accepted are drained. A PUBLISH_DONE for a
/// subscription already waiting is ignored.
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
                // Requests whose countdown is in `expiries`.
                let mut waiting = HashSet::new();
                loop {
                    tokio::select! {
                        Some(publish_done) = publish_done_receiver.recv() => {
                            let request_id = publish_done.request_id();
                            if !waiting.insert(request_id) {
                                continue;
                            }
                            let countdown = Countdown::look_up(
                                connection.upgrade(),
                                request_id,
                                publish_done.stream_count(),
                            )
                            .await;
                            let ended = (session_context.upgrade(), connection.upgrade());
                            expiries.push(async move {
                                if let (Some(session), Some(connection)) = ended {
                                    session.forget_ended_subscription(&connection, request_id).await;
                                }
                                let _ = executor::timeout(PUBLISH_DONE_GRACE_PERIOD, countdown.counted())
                                    .await;
                                request_id
                            });
                        }
                        Some(request_id) = expiries.next() => {
                            waiting.remove(&request_id);
                            let (Some(session), Some(connection)) =
                                (session_context.upgrade(), connection.upgrade())
                            else {
                                break;
                            };
                            tracing::debug!(request_id, "ending the subscription PUBLISH_DONE terminated");
                            session.end_subscription(&connection, request_id).await;
                        }
                        else => break,
                    }
                }
            }
            .instrument(session_span),
        )
    }
}

/// What ends a PUBLISH_DONE's wait before `PUBLISH_DONE_GRACE_PERIOD` does.
enum Countdown {
    /// Ends once `arrived` reaches the Stream Count `needed`, or once the
    /// subscription's sink is gone.
    Counting {
        arrived: watch::Receiver<u64>,
        needed: u64,
    },
    /// Grace only: a Stream Count of 0 (sent by a datagram track, whose
    /// datagrams can still follow PUBLISH_DONE), no sink receiver installed
    /// for the request (not registered yet, or already cancelled), or no
    /// connection.
    GraceOnly,
}

impl Countdown {
    async fn look_up(
        connection: Option<Arc<Connection>>,
        request_id: RequestId,
        needed: u64,
    ) -> Self {
        if needed == 0 {
            return Self::GraceOnly;
        }
        let Some(connection) = connection else {
            return Self::GraceOnly;
        };
        match connection.arrived_streams(request_id).await {
            Some(arrived) => Self::Counting { arrived, needed },
            None => Self::GraceOnly,
        }
    }

    /// Never resolves for a grace-only countdown.
    async fn counted(self) {
        match self {
            Self::Counting {
                mut arrived,
                needed,
            } => {
                // Fails once the sink is gone, which ends the wait too.
                let _ = arrived.wait_for(|count| *count >= needed).await;
            }
            Self::GraceOnly => std::future::pending().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::FutureExt;
    use tokio::{
        sync::{mpsc::error::TryRecvError, watch},
        time::Instant,
    };

    use super::{Countdown, PUBLISH_DONE_GRACE_PERIOD};
    use crate::{
        Session, SessionEvent, Subscription, Version,
        modules::{
            moqt::{
                control_plane::error_codes::PublishDoneCode,
                runtime::dispatch::incoming_object::{IncomingObject, IncomingObjectReceiver},
            },
            test_support::{
                DRAFTS, connected_sessions, next_event, open_subgroup, send_payload,
                send_without_waiting, take_data_receiver, wait_for_control_delivery, within,
            },
        },
        object::ContentExists,
        subscribe::SubscribeOption,
    };

    /// A subscription the server made to the client's `ns/track`, with
    /// `receiver` the server's object receiver, or its unclaimed subscription.
    struct ServedSubscription<R = IncomingObjectReceiver> {
        client: Session,
        server: Session,
        served: Subscription,
        object_receiver: R,
    }

    async fn served_subscription(name: &str, version: Version) -> ServedSubscription {
        let unclaimed = unclaimed_served_subscription(name, version).await;
        let object_receiver = take_data_receiver(
            &unclaimed.server.subscriber().session,
            unclaimed.object_receiver.track_alias(),
        )
        .await;
        ServedSubscription {
            client: unclaimed.client,
            server: unclaimed.server,
            served: unclaimed.served,
            object_receiver,
        }
    }

    async fn unclaimed_served_subscription(
        name: &str,
        version: Version,
    ) -> ServedSubscription<Subscription> {
        let (client, server) = connected_sessions(name, version).await;
        let mut subscriber = server.subscriber();
        let (subscribed, served) = within(async {
            tokio::join!(
                subscriber.subscribe("ns".into(), "track".into(), SubscribeOption::default()),
                async {
                    let SessionEvent::Subscribe(handler) = next_event(&client).await else {
                        panic!("expected SUBSCRIBE from the server");
                    };
                    let track_alias = handler.ok(0, ContentExists::False).await.unwrap();
                    handler.into_subscription(track_alias)
                }
            )
        })
        .await;
        ServedSubscription {
            client,
            server,
            served,
            object_receiver: subscribed.unwrap(),
        }
    }

    /// Returns once the server has received the PUBLISH_DONE.
    async fn end_with_publish_done<R>(subscription: &ServedSubscription<R>, stream_count: u64) {
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
        let SessionEvent::PublishDone(_) = next_event(&subscription.server).await else {
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
    async fn publish_done_right_behind_subscribe_ok_counts_the_streams_of_the_new_receiver() {
        for version in DRAFTS {
            // Arrange: the server's SUBSCRIBE is answered while its caller is not polled
            let (client, server) = connected_sessions("publish-done-behind-ok", version).await;
            let mut subscriber = server.subscriber();
            let session = subscriber.session.clone();
            let mut subscribed = std::pin::pin!(subscriber.subscribe(
                "ns".into(),
                "track".into(),
                SubscribeOption::default()
            ));
            let SessionEvent::Subscribe(handler) =
                send_without_waiting(&mut subscribed, &client).await
            else {
                panic!("expected SUBSCRIBE from the server");
            };
            let track_alias = handler.ok(0, ContentExists::False).await.unwrap();
            let served = handler.into_subscription(track_alias);
            let publisher = client.publisher();
            let mut group = open_subgroup(&publisher.create_stream(&served), 0).await;
            send_payload(&mut group, b"object").await;
            publisher
                .publish_done(&served, PublishDoneCode::TrackEnded, 1, String::new())
                .await
                .unwrap();
            // Lets the server read PUBLISH_DONE before its caller registers
            // the receiver.
            let _ = tokio::time::timeout(Duration::from_millis(500), server.receive_event()).await;

            // Act
            let subscription = within(subscribed).await.unwrap();
            let mut object_receiver =
                take_data_receiver(&session, subscription.track_alias()).await;
            let arrived = within(object_receiver.recv()).await;
            let ended = tokio::time::timeout(Duration::from_secs(1), object_receiver.recv()).await;

            // Assert: it ends well before the grace period
            assert!(
                matches!(arrived, Some(IncomingObject::Subgroup(_))),
                "{version:?}"
            );
            assert!(matches!(ended, Ok(None)), "{version:?}");
        }
    }

    #[tokio::test]
    async fn objects_of_a_subscription_publish_done_ended_stay_readable_until_it_is_claimed() {
        for version in DRAFTS {
            // Arrange
            let subscription =
                unclaimed_served_subscription("publish-done-unclaimed", version).await;
            let mut group = open_subgroup(
                &subscription
                    .client
                    .publisher()
                    .create_stream(&subscription.served),
                0,
            )
            .await;
            send_payload(&mut group, b"object").await;
            end_with_publish_done(&subscription, 1).await;
            let connection = subscription.server.subscriber().session.connection();
            let request_id = connection
                .wire_request_id(subscription.object_receiver.request_id())
                .unwrap();
            within(async {
                while connection.arrived_streams(request_id).await.is_some() {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await;

            // Act
            let mut object_receiver = take_data_receiver(
                &subscription.server.subscriber().session,
                subscription.object_receiver.track_alias(),
            )
            .await;

            // Assert
            let arrived = within(object_receiver.recv()).await;
            assert!(
                matches!(arrived, Some(IncomingObject::Subgroup(_))),
                "{version:?}"
            );
            assert!(
                within(object_receiver.recv()).await.is_none(),
                "{version:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_subscription_publish_done_ended_sends_no_update_or_unsubscribe() {
        for version in DRAFTS {
            // Arrange
            let subscription =
                unclaimed_served_subscription("publish-done-no-update", version).await;
            end_with_publish_done(&subscription, 0).await;
            let subscriber = subscription.server.subscriber();

            // Act
            let updated = subscriber
                .update_forward(&subscription.object_receiver, false)
                .await;
            let unsubscribed = subscriber.unsubscribe(&subscription.object_receiver).await;

            // Assert: the next message the publisher gets is the marker
            assert!(updated.is_ok(), "{version:?}: {updated:?}");
            assert!(unsubscribed.is_ok(), "{version:?}: {unsubscribed:?}");
            wait_for_control_delivery(&subscription.server, &subscription.client).await;
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

    #[tokio::test]
    async fn stream_count_of_0_is_grace_only() {
        // Act
        let countdown = Countdown::look_up(None, 0, 0).await;

        // Assert
        assert!(matches!(countdown, Countdown::GraceOnly));
        assert!(countdown.counted().now_or_never().is_none());
    }

    #[tokio::test]
    async fn counted_streams_without_a_connection_are_grace_only() {
        // Act
        let countdown = Countdown::look_up(None, 0, 1).await;

        // Assert
        assert!(matches!(countdown, Countdown::GraceOnly));
        assert!(countdown.counted().now_or_never().is_none());
    }

    #[tokio::test]
    async fn counting_ends_once_its_sink_is_gone() {
        // Arrange
        let (sender, arrived) = watch::channel(0);
        let countdown = Countdown::Counting { arrived, needed: 2 };

        // Act
        drop(sender);

        // Assert
        assert!(countdown.counted().now_or_never().is_some());
    }

    #[tokio::test]
    async fn counting_ends_once_the_needed_streams_arrived() {
        // Arrange
        let (sender, arrived) = watch::channel(1);
        let countdown = Countdown::Counting { arrived, needed: 2 };

        // Act
        sender.send_replace(2);

        // Assert
        assert!(countdown.counted().now_or_never().is_some());
    }
}
