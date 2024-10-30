use crate::messages::moqt_payload::MOQTPayload;
use crate::variable_integer::{read_variable_integer_from_buffer, write_variable_integer};
use crate::{
    modules::variable_bytes::write_variable_bytes, variable_bytes::read_variable_bytes_from_buffer,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::any::Any;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SubscribeNamespaceError {
    track_namespace_prefix: Vec<String>,
    error_code: u64,
    reason_phrase: String,
}

impl SubscribeNamespaceError {
    pub fn new(
        track_namespace_prefix: Vec<String>,
        error_code: u64,
        reason_phrase: String,
    ) -> Self {
        SubscribeNamespaceError {
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

impl MOQTPayload for SubscribeNamespaceError {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
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

        tracing::trace!("Depacketized subscribe namespace error message.");

        Ok(SubscribeNamespaceError {
            track_namespace_prefix: track_namespace_prefix_tuple,
            error_code,
            reason_phrase,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        // Track Namespace Prefix Number of elements
        let track_namespace_prefix_tuple_length = self.track_namespace_prefix.len();
        buf.extend(write_variable_integer(
            track_namespace_prefix_tuple_length as u64,
        ));
        for track_namespace_prefix in &self.track_namespace_prefix {
            // Track Namespace Prefix
            buf.extend(write_variable_bytes(
                &track_namespace_prefix.as_bytes().to_vec(),
            ));
        }
        // Error Code
        buf.extend(write_variable_integer(self.error_code));
        //　Reason Phrase
        buf.extend(write_variable_bytes(
            &self.reason_phrase.as_bytes().to_vec(),
        ));

        tracing::trace!("Packetized subscribe namespace error message.");
    }
    /// Method to enable downcasting from MOQTPayload to SubscribeNamespaceError
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::messages::moqt_payload::MOQTPayload;
    use crate::modules::messages::control_messages::subscribe_namespace_error::SubscribeNamespaceError;
    use bytes::BytesMut;

    #[test]
    fn packetize() {
        let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
        let error_code: u64 = 1;
        let reason_phrase = "subscribe namespace overlap".to_string();
        let subscribe_namespace_error = SubscribeNamespaceError::new(
            track_namespace_prefix.clone(),
            error_code,
            reason_phrase.clone(),
        );
        let mut buf = BytesMut::new();
        subscribe_namespace_error.packetize(&mut buf);

        let expected_bytes_array = [
            2, // Track Namespace Prefix(tuple): Number of elements
            4, // Track Namespace Prefix(b): Length
            116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
            4,   // Track Namespace Prefix(b): Length
            116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
            1,   // Error Code (i)
            27,  // Reason Phrase (b): length
            115, 117, 98, 115, 99, 114, 105, 98, 101, 32, 110, 97, 109, 101, 115, 112, 97, 99, 101,
            32, 111, 118, 101, 114, 108, 97,
            112, // Reason Phrase (b): Value("subscribe namespace overlap")
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
            115, 117, 98, 115, 99, 114, 105, 98, 101, 32, 110, 97, 109, 101, 115, 112, 97, 99, 101,
            32, 111, 118, 101, 114, 108, 97,
            112, // Reason Phrase (b): Value("subscribe namespace overlap")
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let subscribe_namespace_error = SubscribeNamespaceError::depacketize(&mut buf).unwrap();

        let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
        let error_code: u64 = 1;
        let reason_phrase = "subscribe namespace overlap".to_string();
        let expected_subscribe_namespace_error = SubscribeNamespaceError::new(
            track_namespace_prefix.clone(),
            error_code,
            reason_phrase.clone(),
        );

        assert_eq!(
            subscribe_namespace_error,
            expected_subscribe_namespace_error
        );
    }
}
