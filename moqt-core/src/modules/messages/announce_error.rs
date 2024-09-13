use anyhow::{Context, Result};
use serde::Serialize;
use std::any::Any;

use crate::{
    modules::{variable_bytes::write_variable_bytes, variable_integer::write_variable_integer},
    variable_bytes::{read_fixed_length_bytes_from_buffer, read_variable_bytes_from_buffer},
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
    pub fn new(track_namespace: String, error_code: u64, reason_phrase: String) -> Self {
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
        let track_namespace = String::from_utf8(read_fixed_length_bytes_from_buffer(
            buf,
            track_namespace_length as usize,
        )?)
        .context("track namespace")?;
        let error_code = read_variable_integer_from_buffer(buf).context("error code")?;
        let reason_phrase =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("reason phrase")?;

        tracing::trace!("Depacketized Announce Error message.");

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

        tracing::trace!("Packetized Announce Error message.");
    }
    /// Method to enable downcasting from MOQTPayload to AnnounceError
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::{
        messages::moqt_payload::MOQTPayload, modules::messages::announce_error::AnnounceError,
    };
    use bytes::BytesMut;
    #[test]
    fn packetize_announce_error() {
        let track_namespace = "test".to_string();
        let error_code: u64 = 1;
        let reason_phrase = "already exist".to_string();

        let announce_error =
            AnnounceError::new(track_namespace.clone(), error_code, reason_phrase.clone());
        let mut buf = bytes::BytesMut::new();
        announce_error.packetize(&mut buf);
        let expected_bytes_array = [
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            1,   // Error Code (i)
            13,  // Reason Phrase (b): length
            97, 108, 114, 101, 97, 100, 121, 32, 101, 120, 105, 115,
            116, // Reason Phrase (b): Value("already exist")
        ];
        assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
    }

    #[test]
    fn depacketize_announce_error() {
        let bytes_array = [
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            1,   // Error Code (i)
            13,  // Reason Phrase (b): length
            97, 108, 114, 101, 97, 100, 121, 32, 101, 120, 105, 115,
            116, // Reason Phrase (b): Value("already exist")
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_announce_error = AnnounceError::depacketize(&mut buf).unwrap();

        let track_namespace = "test".to_string();
        let error_code: u64 = 1;
        let reason_phrase = "already exist".to_string();
        let expected_announce_error =
            AnnounceError::new(track_namespace.clone(), error_code, reason_phrase.clone());

        assert_eq!(depacketized_announce_error, expected_announce_error);
    }
}
