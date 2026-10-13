use std::{sync::Weak, time::Duration};

use futures::{StreamExt, stream::FuturesUnordered};
use tokio::sync::mpsc;
use tracing::{Instrument, Span};

use crate::modules::{
    executor::{self, JoinHandle},
    moqt::{control_plane::enums::RequestId, domains::session_context::SessionContext},
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
                            let Some(session) = session_context.upgrade() else {
                                break;
                            };
                            tracing::debug!(request_id, "ending the subscription PUBLISH_DONE terminated");
                            session.cancel_subscription(request_id).await;
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

    use tokio::sync::mpsc::{UnboundedReceiver, error::TryRecvError};

    use super::PUBLISH_DONE_GRACE_PERIOD;
    use crate::{
        Session, SessionEvent,
        modules::{
            moqt::{
                control_plane::control_messages::messages::publish_done::status_code,
                runtime::dispatch::incoming_object::IncomingObject,
            },
            test_support::{register_and_take_data_receiver, spawn_connected_dual_sessions},
        },
    };

    const REQUEST_ID: u64 = 1;
    const TRACK_ALIAS: u64 = 5;

    struct EndedSubscription {
        _client: Session,
        _server: Session,
        object_receiver: UnboundedReceiver<IncomingObject>,
    }

    /// Returns with tokio time paused, so the test's sleeps advance the clock
    /// without waiting.
    async fn subscription_ended_by_publish_done(name: &str) -> EndedSubscription {
        let (client, server) = spawn_connected_dual_sessions(name).await;
        let object_receiver =
            register_and_take_data_receiver(&server.subscriber().session, REQUEST_ID, TRACK_ALIAS)
                .await;
        client
            .publisher()
            .publish_done(REQUEST_ID, status_code::TRACK_ENDED, 0, String::new())
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
        }
    }

    #[tokio::test]
    async fn subscription_stays_open_during_the_grace_period() {
        // Arrange
        let mut subscription = subscription_ended_by_publish_done("publish-done-grace").await;

        // Act
        tokio::time::sleep(PUBLISH_DONE_GRACE_PERIOD - Duration::from_millis(1)).await;

        // Assert
        assert!(matches!(
            subscription.object_receiver.try_recv(),
            Err(TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn subscription_ends_after_the_grace_period() {
        // Arrange
        let mut subscription = subscription_ended_by_publish_done("publish-done-expiry").await;

        // Act
        tokio::time::sleep(PUBLISH_DONE_GRACE_PERIOD).await;

        // Assert
        let received =
            tokio::time::timeout(Duration::from_secs(1), subscription.object_receiver.recv()).await;
        assert!(matches!(received, Ok(None)));
    }
}
