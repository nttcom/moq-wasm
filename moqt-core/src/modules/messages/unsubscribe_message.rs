use anyhow::{Context, Result};

use crate::{
    modules::variable_bytes::read_variable_bytes_from_buffer, variable_bytes::write_variable_bytes,
};

use super::moqt_payload::MOQTPayload;

#[derive(Debug, Clone, PartialEq)]
pub struct UnsubscribeMessage {
    track_namespace: String,
    track_name: String,
}

impl UnsubscribeMessage {
    pub fn new(track_namespace: String, track_name: String) -> UnsubscribeMessage {
        UnsubscribeMessage {
            track_namespace,
            track_name,
        }
    }

    // TODO: 未実装のためallow dead codeをつけている
    #[allow(dead_code)]
    pub(crate) fn track_namespace(&self) -> &str {
        &self.track_namespace
    }

    // TODO: 未実装のためallow dead codeをつけている
    #[allow(dead_code)]
    pub(crate) fn track_name(&self) -> &str {
        &self.track_name
    }
}

impl MOQTPayload for UnsubscribeMessage {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let track_namespace = read_variable_bytes_from_buffer(buf).context("track namespace")?;
        let track_name = read_variable_bytes_from_buffer(buf).context("track name")?;

        let unsubscribe_message = UnsubscribeMessage {
            track_namespace: String::from_utf8(track_namespace)?,
            track_name: String::from_utf8(track_name)?,
        };

        Ok(unsubscribe_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));
        buf.extend(write_variable_bytes(&self.track_name.as_bytes().to_vec()));
    }
}
