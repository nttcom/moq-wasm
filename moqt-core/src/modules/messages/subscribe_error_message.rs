use std::any::Any;

use anyhow::Context;
use serde::Serialize;

use crate::modules::{
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

use super::moqt_payload::MOQTPayload;

#[derive(Debug, Serialize, Clone)]
pub struct SubscribeError {
    track_namespace: String,
    track_name: String,
    error_code: u64,
    reason_phrase_length: u64,
    reason_phrase: String,
}

impl SubscribeError {
    pub fn new(
        track_namespace: String,
        track_name: String,
        error_code: u64,
        reason_phrase: String,
    ) -> SubscribeError {
        let reason_phrase_length = reason_phrase.len() as u64;

        SubscribeError {
            track_namespace,
            track_name,
            error_code,
            reason_phrase_length,
            reason_phrase,
        }
    }
}

impl MOQTPayload for SubscribeError {
    fn depacketize(buf: &mut bytes::BytesMut) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let track_namespace =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("track namespace")?;
        let track_name =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("track name")?;
        let error_code = read_variable_integer_from_buffer(buf).context("error code")?;
        let reason_phrase_length =
            read_variable_integer_from_buffer(buf).context("reason phrase length")?;
        let reason_phrase =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("reason phrase")?;

        Ok(SubscribeError {
            track_namespace,
            track_name,
            error_code,
            reason_phrase_length,
            reason_phrase,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));
        buf.extend(write_variable_bytes(&self.track_name.as_bytes().to_vec()));
        buf.extend(write_variable_integer(self.error_code));
        buf.extend(write_variable_integer(self.reason_phrase_length));
        buf.extend(write_variable_bytes(
            &self.reason_phrase.as_bytes().to_vec(),
        ));
    }
    /// MOQTPayloadからSubscribeErrorへのダウンキャストを可能にするためのメソッド
    fn as_any(&self) -> &dyn Any {
        self
    }
}
