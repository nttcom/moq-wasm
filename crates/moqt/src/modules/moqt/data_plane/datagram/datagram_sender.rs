use crate::modules::{
    executor,
    moqt::{data_plane::object::canonical_object::Object, domains::send_targets::SendTargets},
    transport::transport_connection::DatagramDelivery,
};

pub struct DatagramSender {
    targets: SendTargets,
}

impl DatagramSender {
    pub(crate) fn new(targets: SendTargets) -> Self {
        Self { targets }
    }

    /// Sends `object` as one datagram on every connection the track goes to,
    /// ignoring its forwarding preference. Fails when no connection took it
    /// and none dropped it as too large.
    pub async fn send_object(&mut self, object: &Object) -> anyhow::Result<DatagramDelivery> {
        let results: Vec<_> = self
            .targets
            .resolve()
            .into_iter()
            .map(|(connection, track_alias)| {
                connection
                    .control
                    .version()
                    .encode_datagram(track_alias, object)
                    .and_then(|bytes| connection.transport.send_datagram(bytes))
            })
            .collect();
        executor::yield_now().await;
        let delivery = best_delivery(results)?;
        if delivery == DatagramDelivery::TooLarge {
            tracing::debug!(
                location = ?object.location,
                payload_len = object.payload.len(),
                "dropping a datagram larger than the session allows"
            );
        }
        Ok(delivery)
    }
}

/// `Sent` when any connection took the datagram, else `TooLarge` when any
/// dropped it; otherwise the first failure, or an error when there was no
/// connection.
fn best_delivery(
    results: Vec<anyhow::Result<DatagramDelivery>>,
) -> anyhow::Result<DatagramDelivery> {
    for wanted in [DatagramDelivery::Sent, DatagramDelivery::TooLarge] {
        if results
            .iter()
            .any(|result| matches!(result, Ok(delivery) if *delivery == wanted))
        {
            return Ok(wanted);
        }
    }
    results
        .into_iter()
        .next()
        .unwrap_or_else(|| Err(anyhow::anyhow!("the track has no connection")))
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::modules::test_support::{
        HANDSHAKE_TIMEOUT, connect_sessions, data_receiver, datagram_object, next_datagram,
        published_track_between, spawn_dual_server,
    };
    use crate::object::DatagramDelivery;

    /// An object larger than the session's maximum datagram size is dropped
    /// without notice to the peer, the sender learns it, and the track goes
    /// on.
    async fn assert_an_oversized_datagram_is_dropped(name: &str, scheme: &str, path: &str) {
        // Arrange
        let (port, accept) = spawn_dual_server(name);
        let (client, server) =
            connect_sessions(&format!("{scheme}://127.0.0.1:{port}{path}"), accept)
                .await
                .unwrap();
        let track = published_track_between(client, server).await;
        let mut receiver = data_receiver(&track.server, &track.accepted).await;
        let mut sender = track.client.publisher().create_datagram(&track.published);
        let mut oversized = datagram_object();
        oversized.payload = Bytes::from(vec![0; 64 * 1024]);
        let mut next = datagram_object();
        next.location.object_id = 1;

        // Act
        let dropped = sender.send_object(&oversized).await;
        let sent = sender.send_object(&next).await;

        // Assert: draft-14 §10.3, draft-18 §11.3
        assert_eq!(dropped.unwrap(), DatagramDelivery::TooLarge);
        assert_eq!(sent.unwrap(), DatagramDelivery::Sent);
        let received = tokio::time::timeout(HANDSHAKE_TIMEOUT, next_datagram(&mut receiver))
            .await
            .unwrap();
        assert_eq!(received, next);
    }

    #[tokio::test]
    async fn an_oversized_datagram_is_dropped_over_quic() {
        assert_an_oversized_datagram_is_dropped("datagram-oversized-quic", "moqt", "").await;
    }

    #[tokio::test]
    async fn an_oversized_datagram_is_dropped_over_web_transport() {
        assert_an_oversized_datagram_is_dropped("datagram-oversized-wt", "https", "/moq").await;
    }
}
