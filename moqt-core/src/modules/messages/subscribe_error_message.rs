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
    reason_phrase: String,
}

impl MOQTPayload for SubscribeError {
    fn depacketize(buf: &mut bytes::BytesMut) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let track_namespace = String::from_utf8(read_variable_bytes_from_buffer(buf)?)?;
        let track_name = String::from_utf8(read_variable_bytes_from_buffer(buf)?)?;
        let error_code = read_variable_integer_from_buffer(buf)?;
        let reason_phrase = String::from_utf8(read_variable_bytes_from_buffer(buf)?)?;

        Ok(SubscribeError {
            track_namespace,
            track_name,
            error_code,
            reason_phrase,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));
        buf.extend(write_variable_bytes(&self.track_name.as_bytes().to_vec()));
        buf.extend(write_variable_integer(self.error_code));
        buf.extend(write_variable_bytes(
            &self.reason_phrase.as_bytes().to_vec(),
        ));
    }
}
