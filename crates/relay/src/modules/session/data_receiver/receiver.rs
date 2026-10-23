use moqt::object::{Object, TrackData};

use crate::modules::session::data_receiver::stream_receiver::StreamReceiver;

/// What an upstream subscription delivers next.
pub(crate) enum UpstreamData {
    Subgroup(Box<dyn StreamReceiver>),
    Datagram(Object),
}

#[async_trait::async_trait]
pub(crate) trait DataReceiver: Send + 'static {
    /// A `moqt::object::MalformedTrackError` reports a malformed track; any
    /// other error, that the subscription ended.
    async fn next(&mut self) -> anyhow::Result<UpstreamData>;
}

#[async_trait::async_trait]
impl DataReceiver for moqt::object::DataReceiver {
    async fn next(&mut self) -> anyhow::Result<UpstreamData> {
        Ok(match moqt::object::DataReceiver::next(self).await? {
            TrackData::Subgroup(stream) => UpstreamData::Subgroup(Box::new(stream)),
            TrackData::Datagram(object) => UpstreamData::Datagram(object),
        })
    }
}
