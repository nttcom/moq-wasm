use crate::modules::{
    executor,
    moqt::{data_plane::object::canonical_object::Object, domains::send_targets::SendTargets},
};

pub struct DatagramSender {
    /// The track alias the application knows.
    pub track_alias: u64,
    targets: SendTargets,
}

impl DatagramSender {
    pub(crate) fn new(track_alias: u64, targets: SendTargets) -> Self {
        Self {
            track_alias,
            targets,
        }
    }

    /// Sends `object` as one datagram of this track on every connection it
    /// goes to; its forwarding preference is not read. Fails when no
    /// connection took it.
    pub async fn send_object(&mut self, object: &Object) -> anyhow::Result<()> {
        let mut sent = false;
        let mut failure = None;
        for (connection, track_alias) in self.targets.resolve() {
            let result = connection
                .control
                .version()
                .encode_datagram(track_alias, object)
                .and_then(|bytes| connection.transport.send_datagram(bytes));
            match result {
                Ok(()) => sent = true,
                Err(error) => failure = Some(error),
            }
        }
        executor::yield_now().await;
        match (sent, failure) {
            (true, _) => Ok(()),
            (false, Some(error)) => Err(error),
            (false, None) => anyhow::bail!("the track has no connection"),
        }
    }
}
