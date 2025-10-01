use crate::modules::moqt::messages::{
    control_messages::util::{add_payload_length, validate_payload_length},
    moqt_message::MOQTMessage,
    moqt_message_error::MOQTMessageError,
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
use anyhow::{Context, Result};
use bytes::BytesMut;
use serde::Serialize;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SubscribeNamespaceError {
    request_id: u64,
    track_namespace_prefix: Vec<String>,
    error_code: u64,
    reason_phrase: String,
}

impl SubscribeNamespaceError {
    pub fn new(
        request_id: u64,
        track_namespace_prefix: Vec<String>,
        error_code: u64,
        reason_phrase: String,
    ) -> Self {
        SubscribeNamespaceError {
            request_id,
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

impl MOQTMessage for SubscribeNamespaceError {
    fn depacketize(buf: &mut BytesMut) -> Result<Self, MOQTMessageError> {
        if !validate_payload_length(buf) {
            return Err(MOQTMessageError::ProtocolViolation);
        }

        let request_id = match read_variable_integer_from_buffer(buf) {
            Ok(v) => v,
            Err(_) => return Err(MOQTMessageError::ProtocolViolation),
        };

        let track_namespace_prefix_tuple_length = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("track namespace prefix length")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let mut track_namespace_prefix_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_prefix_tuple_length {
            let track_namespace_prefix = String::from_utf8(
                read_variable_bytes_from_buffer(buf)
                    .map_err(|_| MOQTMessageError::ProtocolViolation)?,
            )
            .context("track namespace prefix")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
            track_namespace_prefix_tuple.push(track_namespace_prefix);
        }
        let error_code = read_variable_integer_from_buffer(buf)
            .context("error code")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let reason_phrase = String::from_utf8(
            read_variable_bytes_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("reason phrase")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;

        Ok(SubscribeNamespaceError {
            request_id,
            track_namespace_prefix: track_namespace_prefix_tuple,
            error_code,
            reason_phrase,
        })
    }

    fn packetize(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.extend(write_variable_integer(self.request_id));
        let track_namespace_prefix_tuple_length = self.track_namespace_prefix.len();
        payload.extend(write_variable_integer(
            track_namespace_prefix_tuple_length as u64,
        ));
        for track_namespace_prefix in &self.track_namespace_prefix {
            payload.extend(write_variable_bytes(
                &track_namespace_prefix.as_bytes().to_vec(),
            ));
        }
        payload.extend(write_variable_integer(self.error_code));
        payload.extend(write_variable_bytes(
            &self.reason_phrase.as_bytes().to_vec(),
        ));
        add_payload_length(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::messages::{
            control_messages::subscribe_namespace_error::SubscribeNamespaceError,
            moqt_message::MOQTMessage,
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let request_id = 0;
            let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
            let error_code: u64 = 1;
            let reason_phrase = "subscribe announces overlap".to_string();
            let subscribe_announces_error = SubscribeNamespaceError::new(
                request_id,
                track_namespace_prefix.clone(),
                error_code,
                reason_phrase.clone(),
            );
            let buf = subscribe_announces_error.packetize();

            let expected_bytes_array = [
                41, // Message Length(i)
                0,  // Request ID(i)
                2,  // Track Namespace Prefix(tuple): Number of elements
                4,  // Track Namespace Prefix(b): Length
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
                41, // Message Length(i)
                0,  // Request ID(i)
                2,  // Track Namespace Prefix(tuple): Number of elements
                4,  // Track Namespace Prefix(b): Length
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
            let subscribe_announces_error = SubscribeNamespaceError::depacketize(&mut buf).unwrap();

            let request_id = 0;
            let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
            let error_code: u64 = 1;
            let reason_phrase = "subscribe announces overlap".to_string();
            let expected_subscribe_announces_error = SubscribeNamespaceError::new(
                request_id,
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
