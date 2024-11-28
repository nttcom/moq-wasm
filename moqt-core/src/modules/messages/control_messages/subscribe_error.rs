use anyhow::Context;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::Serialize;
use std::any::Any;

use crate::{
    messages::moqt_payload::MOQTPayload,
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

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

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SubscribeError {
    subscribe_id: u64,
    error_code: SubscribeErrorCode,
    reason_phrase: String,
    track_alias: u64,
}

impl SubscribeError {
    pub fn new(
        subscribe_id: u64,
        error_code: SubscribeErrorCode,
        reason_phrase: String,
        track_alias: u64,
    ) -> SubscribeError {
        SubscribeError {
            subscribe_id,
            error_code,
            reason_phrase,
            track_alias,
        }
    }

    pub fn subscribe_id(&self) -> u64 {
        self.subscribe_id
    }

    pub fn error_code(&self) -> SubscribeErrorCode {
        self.error_code
    }

    pub fn reason_phrase(&self) -> &String {
        &self.reason_phrase
    }

    pub fn track_alias(&self) -> u64 {
        self.track_alias
    }
}

impl MOQTPayload for SubscribeError {
    fn depacketize(buf: &mut bytes::BytesMut) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let subscribe_id = read_variable_integer_from_buffer(buf).context("subscribe id")?;

        let error_code_u64 = read_variable_integer_from_buffer(buf)?;
        let error_code =
            SubscribeErrorCode::try_from(error_code_u64 as u8).context("error code")?;
        let reason_phrase =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("reason phrase")?;

        let track_alias = read_variable_integer_from_buffer(buf).context("track alias")?;

        Ok(SubscribeError {
            subscribe_id,
            error_code,
            reason_phrase,
            track_alias,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_integer(self.subscribe_id));
        buf.extend(write_variable_integer(u8::from(self.error_code) as u64));
        buf.extend(write_variable_bytes(
            &self.reason_phrase.as_bytes().to_vec(),
        ));
        buf.extend(write_variable_integer(self.track_alias));
    }
    /// Method to enable downcasting from MOQTPayload to SubscribeError
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use bytes::BytesMut;

    use crate::messages::{
        control_messages::subscribe_error::{SubscribeError, SubscribeErrorCode},
        moqt_payload::MOQTPayload,
    };

    #[test]
    fn packetize() {
        let subscribe_id = 0;
        let error_code = SubscribeErrorCode::InvalidRange;
        let reason_phrase = "error".to_string();
        let track_alias = 1;
        let subscribe_error =
            SubscribeError::new(subscribe_id, error_code, reason_phrase.clone(), track_alias);
        let mut buf = BytesMut::new();
        subscribe_error.packetize(&mut buf);

        let expected_bytes_array = [
            0, // Subscribe ID (i)
            1, // Error Code (i)
            5, // Reason Phrase Length (i)
            101, 114, 114, 111, 114, // Reason Phrase (...): Value("error")
            1,   // Track Alias (i)
        ];
        assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
    }

    #[test]
    fn depacketize() {
        let bytes_array = [
            0, // Subscribe ID (i)
            2, // Error Code (i)
            5, // Reason Phrase Length (i)
            101, 114, 114, 111, 114, // Reason Phrase (...): Value("error")
            1,   // Track Alias (i)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let subscribe_error = SubscribeError::depacketize(&mut buf).unwrap();

        let subscribe_id = 0;
        let error_code = SubscribeErrorCode::RetryTrackAlias;
        let reason_phrase = "error".to_string();
        let track_alias = 1;
        let expected_subscribe_error =
            SubscribeError::new(subscribe_id, error_code, reason_phrase.clone(), track_alias);

        assert_eq!(subscribe_error, expected_subscribe_error);
    }
}
