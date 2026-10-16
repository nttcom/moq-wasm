use crate::modules::{
    executor,
    moqt::{
        data_plane::object::canonical_object::Object,
        domains::send_targets::{SendTargets, any_sent},
    },
};

pub struct DatagramSender {
    targets: SendTargets,
}

impl DatagramSender {
    pub(crate) fn new(targets: SendTargets) -> Self {
        Self { targets }
    }

    /// Sends `object` as one datagram on every connection the track goes to,
    /// ignoring its forwarding preference. Fails when no connection took it.
    pub async fn send_object(&mut self, object: &Object) -> anyhow::Result<()> {
        let results = self
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
        any_sent(results)
    }
}
