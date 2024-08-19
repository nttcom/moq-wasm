use super::moqt_payload::MOQTPayload;
use crate::{
    modules::variable_bytes::read_variable_bytes_from_buffer, variable_bytes::write_variable_bytes,
};
use anyhow::{Context, Result};
use std::any::Any;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UnAnnounceMessage {
    track_namespace: String,
}

impl UnAnnounceMessage {
    pub(crate) fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
}

impl MOQTPayload for UnAnnounceMessage {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let track_namespace = read_variable_bytes_from_buffer(buf).context("track namespace")?;

        let unannounce_message = UnAnnounceMessage {
            track_namespace: String::from_utf8(track_namespace)?,
        };

        Ok(unannounce_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ))
    }
    /// Method to enable downcasting from MOQTPayload to UnAnnounceMessage
    fn as_any(&self) -> &dyn Any {
        self
    }
}
