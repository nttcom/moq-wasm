use crate::modules::{
    variable_bytes::write_variable_bytes, variable_integer::write_variable_integer,
};

use super::payload::Payload;

pub(crate) struct AnnounceError {
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

impl Payload for AnnounceError {
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
        buf.extend(write_variable_integer(self.error_code));
        buf.extend(write_variable_bytes(
            &self.reason_phrase.as_bytes().to_vec(),
        ));
    }
}
