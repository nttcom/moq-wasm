use std::error;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::{
    modules::{variable_bytes::write_variable_bytes, variable_integer::write_variable_integer},
    variable_bytes::read_variable_bytes_from_buffer,
    variable_integer::read_variable_integer_from_buffer,
};

use super::moqt_payload::MOQTPayload;

#[derive(Debug, Serialize, Clone)]
pub struct AnnounceError {
    track_namespace: String,
    error_code: u64,
    reason_phrase: String,
}

impl AnnounceError {
    pub(crate) fn new(track_namespace: String, error_code: u64, reason_phrase: String) -> Self {
        AnnounceError {
            track_namespace,
            error_code,
            reason_phrase,
        }
    }
}

impl MOQTPayload for AnnounceError {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self>
    where
        Self: Sized,
    {
        let track_namespace =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("track namespace")?;
        let error_code = read_variable_integer_from_buffer(buf).context("error code")?;
        let reason_phrase =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("reason phrase")?;

        Ok(AnnounceError {
            track_namespace,
            error_code,
            reason_phrase,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));
        buf.extend(write_variable_integer(self.error_code));
        buf.extend(write_variable_bytes(
            &self.reason_phrase.as_bytes().to_vec(),
        ));
    }
}
