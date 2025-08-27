use crate::{
    modules::moqt::messages::moqt_payload::MOQTPayload,
    modules::moqt::messages::variable_bytes::{
        read_variable_bytes_from_buffer, write_variable_bytes,
    },
    modules::moqt::messages::variable_integer::{
        read_variable_integer_from_buffer, write_variable_integer,
    },
};
use anyhow::{Context, Result};
use bytes::BytesMut;
use serde::Serialize;
use std::any::Any;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SubscribeAnnouncesError {
    track_namespace_prefix: Vec<String>,
    error_code: u64,
    reason_phrase: String,
}

impl SubscribeAnnouncesError {
    pub fn new(
        track_namespace_prefix: Vec<String>,
        error_code: u64,
        reason_phrase: String,
    ) -> Self {
        SubscribeAnnouncesError {
            track_namespace_prefix,
            error_code,
            reason_phrase,
        }
    }

    pub fn track_namespace_prefix(&self) -> &Vec<String> {
        &self.track_namespace_prefix
    }

    pub fn error_code(&self) -> u64 {
        self.error_code
    }

    pub fn reason_phrase(&self) -> &String {
        &self.reason_phrase
    }
}

impl MOQTPayload for SubscribeAnnouncesError {
    fn depacketize(buf: &mut BytesMut) -> Result<Self> {
        let track_namespace_prefix_tuple_length =
            u8::try_from(read_variable_integer_from_buffer(buf)?)
                .context("track namespace prefix length")?;
        let mut track_namespace_prefix_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_prefix_tuple_length {
            let track_namespace_prefix = String::from_utf8(read_variable_bytes_from_buffer(buf)?)
                .context("track namespace prefix")?;
            track_namespace_prefix_tuple.push(track_namespace_prefix);
        }
        let error_code = read_variable_integer_from_buffer(buf).context("error code")?;
        let reason_phrase =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("reason phrase")?;

        Ok(SubscribeAnnouncesError {
            track_namespace_prefix: track_namespace_prefix_tuple,
            error_code,
            reason_phrase,
        })
    }

    fn packetize(&self, buf: &mut BytesMut) {
        let track_namespace_prefix_tuple_length = self.track_namespace_prefix.len();
        buf.extend(write_variable_integer(
            track_namespace_prefix_tuple_length as u64,
        ));
        for track_namespace_prefix in &self.track_namespace_prefix {
            buf.extend(write_variable_bytes(
                &track_namespace_prefix.as_bytes().to_vec(),
            ));
        }
        buf.extend(write_variable_integer(self.error_code));
        buf.extend(write_variable_bytes(
            &self.reason_phrase.as_bytes().to_vec(),
        ));
    }
    /// Method to enable downcasting from MOQTPayload to SubscribeAnnouncesError
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::messages::{
            control_messages::subscribe_announces_error::SubscribeAnnouncesError,
            moqt_payload::MOQTPayload,
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
            let error_code: u64 = 1;
            let reason_phrase = "subscribe announces overlap".to_string();
            let subscribe_announces_error = SubscribeAnnouncesError::new(
                track_namespace_prefix.clone(),
                error_code,
                reason_phrase.clone(),
            );
            let mut buf = BytesMut::new();
            subscribe_announces_error.packetize(&mut buf);

            let expected_bytes_array = [
                2, // Track Namespace Prefix(tuple): Number of elements
                4, // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                4,   // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                1,   // Error Code (i)
                27,  // Reason Phrase (b): length
                115, 117, 98, 115, 99, 114, 105, 98, 101, 32, 97, 110, 110, 111, 117, 110, 99, 101,
                115, 32, 111, 118, 101, 114, 108, 97,
                112, // Reason Phrase (b): Value("subscribe announces overlap")
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize() {
            let bytes_array = [
                2, // Track Namespace Prefix(tuple): Number of elements
                4, // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                4,   // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                1,   // Error Code (i)
                27,  // Reason Phrase (b): length
                115, 117, 98, 115, 99, 114, 105, 98, 101, 32, 97, 110, 110, 111, 117, 110, 99, 101,
                115, 32, 111, 118, 101, 114, 108, 97,
                112, // Reason Phrase (b): Value("subscribe announces overlap")
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let subscribe_announces_error = SubscribeAnnouncesError::depacketize(&mut buf).unwrap();

            let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
            let error_code: u64 = 1;
            let reason_phrase = "subscribe announces overlap".to_string();
            let expected_subscribe_announces_error = SubscribeAnnouncesError::new(
                track_namespace_prefix.clone(),
                error_code,
                reason_phrase.clone(),
            );

            assert_eq!(
                subscribe_announces_error,
                expected_subscribe_announces_error
            );
        }
    }
}
