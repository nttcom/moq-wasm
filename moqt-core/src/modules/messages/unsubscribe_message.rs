use anyhow::Result;

use crate::modules::variable_bytes::read_variable_bytes_from_buffer;

use super::moqt_payload::MOQTPayload;

pub(crate) struct UnsubscribeMessage {
    track_namespace: String,
    track_name: String,
}

impl UnsubscribeMessage {
    pub(crate) fn track_namespace(&self) -> &str {
        &self.track_namespace
    }

    pub(crate) fn track_name(&self) -> &str {
        &self.track_name
    }
}

impl MOQTPayload for UnsubscribeMessage {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let track_namespace = read_variable_bytes_from_buffer(buf)?;
        let track_name = read_variable_bytes_from_buffer(buf)?;

        let unsubscribe_message = UnsubscribeMessage {
            track_namespace: String::from_utf8(track_namespace)?,
            track_name: String::from_utf8(track_name)?,
        };

        Ok(unsubscribe_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        todo!()
    }
}
