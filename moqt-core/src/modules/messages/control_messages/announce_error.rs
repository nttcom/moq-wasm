use anyhow::{Context, Result};
use serde::Serialize;
use std::any::Any;

use crate::{
    messages::moqt_payload::MOQTPayload,
    modules::{variable_bytes::write_variable_bytes, variable_integer::write_variable_integer},
    variable_bytes::read_variable_bytes_from_buffer,
    variable_integer::read_variable_integer_from_buffer,
};

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct AnnounceError {
    track_namespace: Vec<String>,
    error_code: u64,
    reason_phrase: String,
}

impl AnnounceError {
    pub fn new(track_namespace: Vec<String>, error_code: u64, reason_phrase: String) -> Self {
        AnnounceError {
            track_namespace,
            error_code,
            reason_phrase,
        }
    }

    pub fn track_namespace(&self) -> &Vec<String> {
        &self.track_namespace
    }

    pub fn error_code(&self) -> u64 {
        self.error_code
    }
}

impl MOQTPayload for AnnounceError {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let track_namespace_tuple_length = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("track namespace length")?;
        let mut track_namespace_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let track_namespace = String::from_utf8(read_variable_bytes_from_buffer(buf)?)
                .context("track namespace")?;
            track_namespace_tuple.push(track_namespace);
        }
        let error_code = read_variable_integer_from_buffer(buf).context("error code")?;
        let reason_phrase =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("reason phrase")?;

        Ok(AnnounceError {
            track_namespace: track_namespace_tuple,
            error_code,
            reason_phrase,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        let track_namespace_tuple_length = self.track_namespace.len() as u64;
        buf.extend(write_variable_integer(track_namespace_tuple_length));
        for track_namespace in &self.track_namespace {
            buf.extend(write_variable_bytes(&track_namespace.as_bytes().to_vec()));
        }
        buf.extend(write_variable_integer(self.error_code));
        buf.extend(write_variable_bytes(
            &self.reason_phrase.as_bytes().to_vec(),
        ));
    }
    /// Method to enable downcasting from MOQTPayload to AnnounceError
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::{
        messages::moqt_payload::MOQTPayload,
        modules::messages::control_messages::announce_error::AnnounceError,
    };
    use bytes::BytesMut;
    #[test]
    fn packetize_announce_error() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let error_code = 1;
        let reason_phrase = "already exist".to_string();

        let announce_error =
            AnnounceError::new(track_namespace.clone(), error_code, reason_phrase.clone());
        let mut buf = bytes::BytesMut::new();
        announce_error.packetize(&mut buf);
        let expected_bytes_array = [
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
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
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            1,   // Error Code (i)
            13,  // Reason Phrase (b): length
            97, 108, 114, 101, 97, 100, 121, 32, 101, 120, 105, 115,
            116, // Reason Phrase (b): Value("already exist")
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_announce_error = AnnounceError::depacketize(&mut buf).unwrap();

        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let error_code: u64 = 1;
        let reason_phrase = "already exist".to_string();
        let expected_announce_error =
            AnnounceError::new(track_namespace.clone(), error_code, reason_phrase.clone());

        assert_eq!(depacketized_announce_error, expected_announce_error);
    }
}
