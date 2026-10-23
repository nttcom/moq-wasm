use std::{sync::Weak, time::Duration};

use futures::{StreamExt, stream::FuturesUnordered};
use tokio::sync::mpsc;
use tracing::{Instrument, Span};

use crate::modules::{
    executor::{self, JoinHandle},
    moqt::{
        control_plane::enums::RequestId,
        domains::{connection::Connection, session_context::SessionContext},
    },
};

/// draft-14 §9.12: PUBLISH_DONE travels on the control stream and can arrive
/// before the subscription's last data streams, so the subscription stays open
/// for this period after PUBLISH_DONE.
pub(crate) const PUBLISH_DONE_GRACE_PERIOD: Duration = Duration::from_secs(10);

/// Cancels a subscription `PUBLISH_DONE_GRACE_PERIOD` after its PUBLISH_DONE,
/// like an unsubscribe but without sending UNSUBSCRIBE. Its `TrackReader` then
/// returns `Ok(None)` once the streams it already accepted are drained.
pub(crate) struct PublishDoneExpiryTask;

impl PublishDoneExpiryTask {
    pub(crate) fn run(
        mut publish_done_receiver: mpsc::UnboundedReceiver<RequestId>,
        session_context: Weak<SessionContext>,
        connection: Weak<Connection>,
        expiry_span: Span,
    ) -> JoinHandle {
        executor::spawn(
            "Publish Done Expiry",
            async move {
                let mut expiries = FuturesUnordered::new();
                loop {
                    tokio::select! {
                        Some(request_id) = publish_done_receiver.recv() => {
                            expiries.push(async move {
                                executor::sleep(PUBLISH_DONE_GRACE_PERIOD).await;
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
            .instrument(expiry_span),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::{
        sync::mpsc::{UnboundedReceiver, error::TryRecvError},
        time::Instant,
    };

    use super::PUBLISH_DONE_GRACE_PERIOD;
    use crate::{
        Session, SessionEvent, Version,
        modules::{
            moqt::{
                control_plane::error_codes::PublishDoneCode,
                runtime::dispatch::incoming_object::IncomingObject,
            },
            test_support::{DRAFTS, connected_sessions, take_data_receiver, within},
        },
        object::ContentExists,
        subscribe::SubscribeOption,
    };

    struct EndedSubscription {
        _client: Session,
        _server: Session,
        object_receiver: UnboundedReceiver<IncomingObject>,
        /// Taken before PUBLISH_DONE is sent, so the grace period ends at least
        /// `PUBLISH_DONE_GRACE_PERIOD` after this instant.
        publish_done_sent_at: Instant,
    }

    /// The server subscribes to the client, which ends the subscription with
    /// PUBLISH_DONE. Returns with tokio time paused, so the test's sleeps
    /// advance the clock without waiting.
    async fn subscription_ended_by_publish_done(name: &str, version: Version) -> EndedSubscription {
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
        let publish_done_sent_at = Instant::now();
        client
            .publisher()
            .publish_done(&served, PublishDoneCode::TrackEnded, 0, String::new())
            .await
            .unwrap();
        let SessionEvent::PublishDone(_) = server.receive_event().await.unwrap() else {
            panic!("expected PUBLISH_DONE from the client");
        };
        tokio::time::pause();
        EndedSubscription {
            _client: client,
            _server: server,
            object_receiver,
            publish_done_sent_at,
        }
    }

    #[tokio::test]
    async fn subscription_stays_open_during_the_grace_period() {
        for version in DRAFTS {
            // Arrange
            let mut subscription =
                subscription_ended_by_publish_done("publish-done-grace", version).await;

            // Act
            tokio::time::sleep_until(
                subscription.publish_done_sent_at + PUBLISH_DONE_GRACE_PERIOD
                    - Duration::from_millis(1),
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
    async fn subscription_ends_after_the_grace_period() {
        for version in DRAFTS {
            // Arrange
            let mut subscription =
                subscription_ended_by_publish_done("publish-done-expiry", version).await;

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
