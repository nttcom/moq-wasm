use std::{
    future::poll_fn,
    sync::{Arc, Mutex},
    task::{Context, Waker},
};

use crate::modules::transport::transport_connection::BoxedSendStream;

/// A send stream shared by its `StreamSender` and, once its FIN is sent,
/// the connection's `UnacknowledgedStreams`.
pub(crate) type SharedSendStream = Arc<tokio::sync::Mutex<BoxedSendStream>>;

/// The data streams of one connection whose FIN was sent and whose data the
/// peer may not have acknowledged yet. Closing the connection discards that
/// data (RFC 9000 §10.2), so `Session::close` waits for them first.
/// `acknowledged` polls each held stream, so it must outlive its sender.
#[derive(Clone, Debug, Default)]
pub(crate) struct UnacknowledgedStreams(Arc<Mutex<Vec<SharedSendStream>>>);

impl UnacknowledgedStreams {
    /// Holds `stream`, whose FIN was just sent, and drops the held streams
    /// already acknowledged, stopped or lost.
    pub(crate) fn hold(&self, stream: SharedSendStream) {
        let mut streams = self.0.lock().expect("unacknowledged streams lock");
        streams.retain(|held| !is_acknowledged(held));
        streams.push(stream);
    }

    /// Takes the held streams and resolves once the peer acknowledged or
    /// stopped each, or the connection is lost.
    pub(crate) async fn acknowledged(&self) {
        let streams = std::mem::take(&mut *self.0.lock().expect("unacknowledged streams lock"));
        for stream in streams {
            let mut stream = stream.lock().await;
            poll_fn(|cx| stream.poll_stopped(cx).map(|_| ())).await;
        }
    }
}

/// A stream a sender is still using counts as unacknowledged.
fn is_acknowledged(stream: &SharedSendStream) -> bool {
    let Ok(mut stream) = stream.try_lock() else {
        return false;
    };
    stream
        .poll_stopped(&mut Context::from_waker(Waker::noop()))
        .is_ready()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::BytesMut;

    use super::*;
    use crate::DEFAULT_VERSIONS;
    use crate::modules::{
        moqt::{data_plane::stream::stream_sender::StreamSender, version::application_protocols},
        test_support::{connect_transports, within},
    };

    #[tokio::test]
    async fn holding_a_stream_forgets_the_ones_already_acknowledged() {
        // Arrange
        let protocols = application_protocols(DEFAULT_VERSIONS).unwrap();
        let (client, _server) = connect_transports(
            "unacknowledged-streams",
            "moqt",
            "",
            protocols.clone(),
            protocols,
        )
        .await;
        let unacknowledged = UnacknowledgedStreams::default();
        let first = StreamSender::held_until_acknowledged(
            client.open_uni().await.unwrap(),
            &unacknowledged,
        );
        first.send(&BytesMut::from(&b"first"[..])).await.unwrap();
        first.close().await.unwrap();
        let held = unacknowledged.0.lock().unwrap()[0].clone();
        within(async {
            while !is_acknowledged(&held) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        drop(held);
        let second = StreamSender::held_until_acknowledged(
            client.open_uni().await.unwrap(),
            &unacknowledged,
        );

        // Act
        second.close().await.unwrap();

        // Assert: only the second stream is held
        assert_eq!(unacknowledged.0.lock().unwrap().len(), 1);
    }
}
