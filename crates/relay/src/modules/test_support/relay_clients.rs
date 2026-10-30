use std::time::Duration;

use moqt::{
    Session, SessionEvent, Subscription,
    publish::{Origin, Retention},
    subscribe::SubscribeOption,
};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::auth::test_support::{RunningRelay, connect_client};

pub(crate) const NAMESPACE: &str = "APP/room";
pub(crate) const TRACK: &str = "video";
const EVENT_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn within<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(EVENT_TIMEOUT, future)
        .await
        .expect("the relay did not answer within the timeout")
}

pub(crate) async fn next_event(session: &Session) -> SessionEvent {
    within(session.receive_event()).await.unwrap()
}

pub(crate) async fn subscribe_namespace(session: &Session, prefix: &str) {
    within(session.subscriber().subscribe_namespace(prefix.to_string()))
        .await
        .unwrap();
}

pub(crate) async fn publish_namespace(session: &Session, namespace: &str) {
    within(session.publisher().publish_namespace(namespace.to_string()))
        .await
        .unwrap();
}

pub(crate) async fn subscribe_track(session: &Session, option: SubscribeOption) -> Subscription {
    let mut subscriber = session.subscriber();
    within(subscriber.subscribe(NAMESPACE.to_string(), TRACK.to_string(), option))
        .await
        .unwrap()
}

/// A client that announces `NAMESPACE` and serves `TRACK` from `origin`,
/// reporting the Request ID of every UNSUBSCRIBE the relay sends it.
pub(crate) struct TrackPublisher {
    pub(crate) origin: Origin,
    unsubscribe_receiver: mpsc::UnboundedReceiver<u64>,
    events: JoinHandle<()>,
}

impl TrackPublisher {
    pub(crate) async fn run(relay: &RunningRelay) -> Self {
        let session = connect_client(relay).await;
        let origin = Origin::default();
        origin
            .ensure_track(NAMESPACE, TRACK, 0, Retention::NewestOnly)
            .await;
        publish_namespace(&session, NAMESPACE).await;
        let (unsubscribe_sender, unsubscribe_receiver) = mpsc::unbounded_channel();
        let serving = origin.clone();
        let events = tokio::spawn(async move {
            while let Ok(event) = session.receive_event().await {
                if let SessionEvent::Unsubscribe(handler) = &event {
                    let _ = unsubscribe_sender.send(handler.subscribe_id());
                }
                serving.handle(event).await;
            }
        });
        Self {
            origin,
            unsubscribe_receiver,
            events,
        }
    }

    pub(crate) async fn next_unsubscribe(&mut self) -> Option<u64> {
        within(self.unsubscribe_receiver.recv()).await
    }
}

impl Drop for TrackPublisher {
    fn drop(&mut self) {
        self.events.abort();
    }
}
