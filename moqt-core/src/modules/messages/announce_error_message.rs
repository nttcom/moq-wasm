use anyhow::{Context, Result};
use serde::Serialize;

use crate::{
    modules::{variable_bytes::write_variable_bytes, variable_integer::write_variable_integer},
    variable_bytes::{
        read_variable_bytes_from_buffer, read_variable_bytes_with_length_from_buffer,
    },
    variable_integer::read_variable_integer_from_buffer,
};

use super::moqt_payload::MOQTPayload;

#[derive(Debug, Serialize, Clone)]
pub struct AnnounceError {
    track_namespace: String,
    error_code: u64,
    reason_phrase: String,
}

// draft-03
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
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let track_namespace_length = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("track namespace length")?;
        let track_namespace = String::from_utf8(read_variable_bytes_with_length_from_buffer(
            buf,
            track_namespace_length as usize,
        )?)
        .context("track namespace")?;
        let error_code = read_variable_integer_from_buffer(buf).context("error_code")?;
        let reason_phrase =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("reason phrase")?;

        Ok(AnnounceError {
            track_namespace,
            error_code,
            reason_phrase,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        /*
            ANNOUNCE_ERROR {
                Track Namespace(b),
                Error Code (i),
                Reason Phrase (b),
            }
        */
        // Track Namespace bytes Length
        buf.extend(write_variable_integer(self.track_namespace.len() as u64));
        // Track Namespace
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));
        // Error Code
        buf.extend(write_variable_integer(self.error_code));
        //　Reason Phrase
        buf.extend(write_variable_bytes(
            &self.reason_phrase.as_bytes().to_vec(),
        ));
    }
}
