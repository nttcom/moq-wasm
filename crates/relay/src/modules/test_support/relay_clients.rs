use std::time::Duration;

use moqt::{Session, SessionEvent};

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
