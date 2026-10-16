use crate::modules::moqt::{
    data_plane::stream::stream_data_sender::{StreamDataSender, WireStream},
    domains::send_targets::SendTargets,
};

pub struct StreamDataSenderFactory {
    targets: SendTargets,
}

impl StreamDataSenderFactory {
    pub(crate) fn new(targets: SendTargets) -> Self {
        Self { targets }
    }

    /// A stream on every connection the track goes to now; a connection a
    /// migration adds takes the track from the next subgroup on. Fails when
    /// no connection opened a stream.
    pub async fn next(&self) -> anyhow::Result<StreamDataSender> {
        let mut streams = Vec::new();
        let mut failure = None;
        for (connection, track_alias) in self.targets.resolve() {
            match connection.transport.open_uni().await {
                Ok(send_stream) => streams.push(WireStream::new(
                    connection.control.version(),
                    track_alias,
                    send_stream,
                )),
                Err(error) => failure = Some(error),
            }
        }
        if streams.is_empty() {
            return Err(failure.unwrap_or_else(|| anyhow::anyhow!("the track has no connection")));
        }
        Ok(StreamDataSender::on_streams(streams))
    }
}
