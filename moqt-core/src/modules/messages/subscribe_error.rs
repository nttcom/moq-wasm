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
    reason_phrase: String,
}

impl SubscribeError {
    pub fn new(
        track_namespace: String,
        track_name: String,
        error_code: u64,
        reason_phrase: String,
    ) -> SubscribeError {
        SubscribeError {
            track_namespace,
            track_name,
            error_code,
            reason_phrase,
        }
    }

    pub fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
    pub fn track_name(&self) -> &str {
        &self.track_name
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
        let reason_phrase =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("reason phrase")?;

        tracing::trace!("Depacketized Subscribe Error message.");

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

        tracing::trace!("Packetized Subscribe Error message.");
    }
    /// Method to enable downcasting from MOQTPayload to SubscribeError
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::{
        messages::moqt_payload::MOQTPayload, modules::messages::subscribe_error::SubscribeError,
    };
    use bytes::BytesMut;

    #[test]
    fn packetize() {
        let track_namespace = "tests".to_string();
        let track_name = "test".to_string();
        let error_code = 1;
        let reason_phrase = "error".to_string();
        let subscribe_error = SubscribeError::new(
            track_namespace.clone(),
            track_name.clone(),
            error_code,
            reason_phrase.clone(),
        );
        let mut buf = BytesMut::new();
        subscribe_error.packetize(&mut buf);

        let expected_bytes_array = [
            5, // track_namespace length
            116, 101, 115, 116, 115, // track_namespace bytes("tests")
            4,   // track_name length
            116, 101, 115, 116, // track_name bytes("test")
            1,   // error_code
            5,   // reason_phrase length
            101, 114, 114, 111, 114, // reason_phrase bytes("error")
        ];
        assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
    }

    #[test]
    fn depacketize() {
        let bytes_array = [
            4, // track_namespace length
            116, 101, 115, 116, // track_namespace bytes("test")
            4,   // track_name length
            116, 101, 115, 116, // track_name bytes("test")
            1,   // error_code
            4,   // reason_phrase length
            116, 101, 115, 116, // reason_phrase bytes("test")
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let subscribe_error = SubscribeError::depacketize(&mut buf).unwrap();

        assert_eq!(subscribe_error.track_namespace(), "test");
        assert_eq!(subscribe_error.track_name(), "test");
    }
}
