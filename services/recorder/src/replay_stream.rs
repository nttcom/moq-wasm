use std::{convert::Infallible, future, time::Duration};

use bytes::Bytes;
use futures_util::{StreamExt, stream};
use http_body_util::{BodyExt, StreamBody, combinators::BoxBody};
use hyper::body::Frame;
use serde_json::Value;
use tokio::time::{Instant, interval_at};

use crate::replay::ReplayLease;

/// Short enough that a proxy does not close the open answer as idle. Each
/// write also lets hyper find a client that went away.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// The body of a `publish` answer: the replay's description as one JSON line,
/// then an empty line every [`KEEPALIVE_INTERVAL`]. It never ends; hyper drops
/// it once the client goes away, and with it the lease.
pub(crate) fn replay_stream(description: &Value, lease: ReplayLease) -> BoxBody<Bytes, Infallible> {
    let description = Bytes::from(format!("{description}\n"));
    let keepalive = interval_at(Instant::now() + KEEPALIVE_INTERVAL, KEEPALIVE_INTERVAL);
    let keepalives = stream::unfold((keepalive, lease), |(mut keepalive, lease)| async move {
        keepalive.tick().await;
        Some((Bytes::from_static(b"\n"), (keepalive, lease)))
    });
    let frames = stream::once(future::ready(description))
        .chain(keepalives)
        .map(|data| Ok(Frame::data(data)));
    BodyExt::boxed(StreamBody::new(frames))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;

    #[tokio::test]
    async fn sends_the_description_first_and_releases_the_lease_when_dropped() {
        // Arrange
        let (lease_sender, mut lease_receiver) = mpsc::channel(1);
        let mut stream = replay_stream(
            &json!({ "namespace_prefix": "anon/room/recordings/id" }),
            ReplayLease::new(lease_sender),
        );

        // Act
        let first = stream.frame().await.unwrap().unwrap().into_data().unwrap();
        drop(stream);

        // Assert
        assert_eq!(
            first,
            Bytes::from_static(b"{\"namespace_prefix\":\"anon/room/recordings/id\"}\n")
        );
        assert!(lease_receiver.recv().await.is_none());
    }
}
