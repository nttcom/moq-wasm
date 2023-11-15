use crate::modules::variable_bytes::write_variable_bytes;

use super::payload::Payload;

pub(crate) struct AnnounceOk {
    track_namespace: String,
}

impl AnnounceOk {
    pub(crate) fn new(track_namespace: String) -> Self {
        Self { track_namespace }
    }
}

impl Payload for AnnounceOk {
    fn depacketize(buf: &mut bytes::BytesMut) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        todo!()
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));
    }
}
