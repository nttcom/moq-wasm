use anyhow::Result;

use crate::modules::variable_bytes::read_variable_bytes_from_buffer;

use super::payload::Payload;

pub(crate) struct UnAnnounceMessage {
    track_namespace: String,
}

impl UnAnnounceMessage {
    pub(crate) fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
}

impl Payload for UnAnnounceMessage {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let track_namespace = read_variable_bytes_from_buffer(buf)?;

        let unannounce_message = UnAnnounceMessage {
            track_namespace: String::from_utf8(track_namespace)?,
        };

        Ok(unannounce_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        todo!()
    }
}
