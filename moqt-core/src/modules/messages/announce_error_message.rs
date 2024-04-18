use anyhow::{Context, Result};
use serde::Serialize;

use crate::{
    modules::{variable_bytes::write_variable_bytes, variable_integer::write_variable_integer},
    variable_bytes::{
        read_length_and_variable_bytes_from_buffer, read_variable_bytes_with_length_from_buffer,
    },
    variable_integer::read_variable_integer_from_buffer,
};

use super::moqt_payload::MOQTPayload;

#[derive(Debug, Serialize, Clone, PartialEq)]
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
        let error_code = read_variable_integer_from_buffer(buf).context("error code")?;
        let reason_phrase = String::from_utf8(read_length_and_variable_bytes_from_buffer(buf)?)
            .context("reason phrase")?;

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
        buf.extend(write_variable_integer(self.reason_phrase.len() as u64));
        buf.extend(write_variable_bytes(
            &self.reason_phrase.as_bytes().to_vec(),
        ));
    }
}

#[cfg(test)]
mod success {
    use crate::{
        messages::moqt_payload::MOQTPayload,
        modules::messages::announce_error_message::AnnounceError,
    };
    #[test]
    fn packetize_announce_error() {
        let track_namespace = "test".to_string();
        let track_namespace_length = track_namespace.len() as u8;
        let error_code: u64 = 1;
        let reason_phrase = "already exist".to_string();

        let announce_error =
            AnnounceError::new(track_namespace.clone(), error_code, reason_phrase.clone());
        let mut buf = bytes::BytesMut::new();
        announce_error.packetize(&mut buf);

        let mut combined_bytes = Vec::from(track_namespace_length.to_be_bytes());
        combined_bytes.extend_from_slice(track_namespace.as_bytes());
        combined_bytes.extend_from_slice(&(error_code as u8).to_be_bytes());
        combined_bytes.extend_from_slice(&(reason_phrase.len() as u8).to_be_bytes());
        combined_bytes.extend_from_slice(reason_phrase.as_bytes());

        assert_eq!(buf.as_ref(), combined_bytes.as_slice());
    }

    #[test]
    fn depacketize_announce_error() {
        let track_namespace = "test".to_string();
        let track_namespace_length = track_namespace.len() as u8;
        let error_code: u64 = 1;
        let reason_phrase = "already exist".to_string();
        let expected_announce_error =
            AnnounceError::new(track_namespace.clone(), error_code, reason_phrase.clone());

        let mut combined_bytes = Vec::from(track_namespace_length.to_be_bytes());
        combined_bytes.extend_from_slice(track_namespace.as_bytes());
        combined_bytes.extend_from_slice(&(error_code as u8).to_be_bytes());
        combined_bytes.extend_from_slice(&(reason_phrase.len() as u8).to_be_bytes());
        combined_bytes.extend_from_slice(reason_phrase.as_bytes());

        let mut buf = bytes::BytesMut::from(combined_bytes.as_slice());
        let depacketized_announce_error = AnnounceError::depacketize(&mut buf).unwrap();

        assert_eq!(depacketized_announce_error, expected_announce_error);
    }
}
