use anyhow::Result;
use serde::Serialize;

use crate::{
    modules::variable_bytes::write_variable_bytes, variable_bytes::read_variable_bytes_from_buffer,
};

use super::moqt_payload::MOQTPayload;

#[derive(Debug, Serialize, Clone)]
pub struct AnnounceOk {
    track_namespace: String,
}

impl AnnounceOk {
    pub(crate) fn new(track_namespace: String) -> Self {
        Self { track_namespace }
    }
}

impl MOQTPayload for AnnounceOk {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self>
    where
        Self: Sized,
    {
        let track_namespace = String::from_utf8(read_variable_bytes_from_buffer(buf)?)?;

        Ok(AnnounceOk { track_namespace })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));
    }
}
