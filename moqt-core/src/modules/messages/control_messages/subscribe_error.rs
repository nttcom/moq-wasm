use std::any::Any;

use anyhow::Context;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::Serialize;

use crate::modules::{
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

use crate::messages::moqt_payload::MOQTPayload;

#[derive(Debug, IntoPrimitive, TryFromPrimitive, Serialize, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum SubscribeErrorCode {
    InternalError = 0x0,
    InvalidRange = 0x1,
    RetryTrackAlias = 0x2,
    TrackDoesNotExist = 0x3,
    Unauthorized = 0x4,
    Timeout = 0x5,
}

#[derive(Debug, Serialize, Clone)]
pub struct SubscribeError {
    track_namespace: Vec<String>,
    track_name: String,
    error_code: SubscribeErrorCode,
    reason_phrase: String,
}

impl SubscribeError {
    pub fn new(
        track_namespace: Vec<String>,
        track_name: String,
        error_code: SubscribeErrorCode,
        reason_phrase: String,
    ) -> SubscribeError {
        SubscribeError {
            track_namespace,
            track_name,
            error_code,
            reason_phrase,
        }
    }

    pub fn track_namespace(&self) -> &Vec<String> {
        &self.track_namespace
    }
    pub fn track_name(&self) -> &str {
        &self.track_name
    }
    pub fn error_code(&self) -> SubscribeErrorCode {
        self.error_code
    }
}

impl MOQTPayload for SubscribeError {
    fn depacketize(buf: &mut bytes::BytesMut) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let track_namespace_tuple_length = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("track namespace length")?;
        let mut track_namespace_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let track_namespace = String::from_utf8(read_variable_bytes_from_buffer(buf)?)
                .context("track namespace")?;
            track_namespace_tuple.push(track_namespace);
        }
        let track_name =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("track name")?;
        let error_code_u64 = read_variable_integer_from_buffer(buf)?;
        let error_code =
            SubscribeErrorCode::try_from(error_code_u64 as u8).context("error code")?;
        let reason_phrase =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("reason phrase")?;

        tracing::trace!("Depacketized Subscribe Error message.");

        Ok(SubscribeError {
            track_namespace: track_namespace_tuple,
            track_name,
            error_code,
            reason_phrase,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        // Track Namespace Number of elements
        let track_namespace_tuple_length = self.track_namespace.len();
        buf.extend(write_variable_integer(track_namespace_tuple_length as u64));
        for track_namespace in &self.track_namespace {
            // Track Namespace
            buf.extend(write_variable_bytes(&track_namespace.as_bytes().to_vec()));
        }
        buf.extend(write_variable_bytes(&self.track_name.as_bytes().to_vec()));
        buf.extend(write_variable_integer(u8::from(self.error_code) as u64));
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
        messages::moqt_payload::MOQTPayload,
        modules::messages::control_messages::subscribe_error::{
            SubscribeError, SubscribeErrorCode,
        },
    };
    use bytes::BytesMut;

    #[test]
    fn packetize() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "test".to_string();
        let error_code = SubscribeErrorCode::InvalidRange;
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
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Name (b): Length
            116, 101, 115, 116, // Track Name (b): Value("test")
            1,   // Error Code (i)
            5,   // Reason Phrase Length (i)
            101, 114, 114, 111, 114, // Reason Phrase (...): Value("error")
        ];
        assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
    }

    #[test]
    fn depacketize() {
        let bytes_array = [
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Name (b): Length
            116, 101, 115, 116, // Track Name (b): Value("test")
            2,   // Error Code (i)
            5,   // Reason Phrase Length (i)
            101, 114, 114, 111, 114, // Reason Phrase (...): Value("error")
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let subscribe_error = SubscribeError::depacketize(&mut buf).unwrap();

        assert_eq!(
            subscribe_error.track_namespace(),
            &Vec::from(["test".to_string(), "test".to_string()])
        );
        assert_eq!(subscribe_error.track_name(), "test");
        assert_eq!(
            subscribe_error.error_code(),
            SubscribeErrorCode::RetryTrackAlias
        );
    }
}
