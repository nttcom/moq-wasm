use std::{
    convert::Infallible,
    pin::Pin,
    task::{Context, Poll, ready},
    time::Duration,
};

use bytes::Bytes;
use hyper::body::{Body, Frame};
use serde_json::Value;
use tokio::time::{Instant, Interval, interval_at};

use crate::replay::ReplayLease;

/// Short enough that a proxy does not close the open answer as idle. Each
/// write also lets hyper find a client that went away.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// The body of a `publish` answer: the replay's description as one JSON line,
/// then an empty line every [`KEEPALIVE_INTERVAL`]. It never ends; hyper drops
/// it once the client goes away, and with it the lease.
pub(crate) struct ReplayStream {
    description: Option<Bytes>,
    keepalive: Interval,
    _lease: ReplayLease,
}

impl ReplayStream {
    pub(crate) fn new(description: &Value, lease: ReplayLease) -> Self {
        Self {
            description: Some(Bytes::from(format!("{description}\n"))),
            keepalive: interval_at(Instant::now() + KEEPALIVE_INTERVAL, KEEPALIVE_INTERVAL),
            _lease: lease,
        }
    }
}

impl Body for ReplayStream {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        let stream = self.get_mut();
        if let Some(description) = stream.description.take() {
            return Poll::Ready(Some(Ok(Frame::data(description))));
        }
        ready!(stream.keepalive.poll_tick(context));
        Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b"\n")))))
    }
}

#[cfg(test)]
mod tests {
    use http_body_util::BodyExt;
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;

    #[tokio::test]
    async fn sends_the_description_first_and_releases_the_lease_when_dropped() {
        // Arrange
        let (lease_sender, mut lease_receiver) = mpsc::channel(1);
        let mut stream =
            ReplayStream::new(&json!({ "tracks": [] }), ReplayLease::new(lease_sender));

        // Act
        let first = stream.frame().await.unwrap().unwrap().into_data().unwrap();
        drop(stream);

        // Assert
        assert_eq!(first, Bytes::from_static(b"{\"tracks\":[]}\n"));
        assert!(lease_receiver.recv().await.is_none());
    }
}
