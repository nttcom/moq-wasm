use crate::modules::moqt::messages::{
    control_message_type::ControlMessageType,
    control_messages::util::{add_header, validate_header},
    moqt_message::MOQTMessage,
    moqt_message_error::MOQTMessageError,
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
use anyhow::{Context, Result};
use bytes::BytesMut;
use serde::Serialize;
use std::any::Any;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct PublishNamespaceError {
    pub(crate) request_id: u64,
    track_namespace: Vec<String>,
    error_code: u64,
    reason_phrase: String,
}

impl PublishNamespaceError {
    pub fn new(
        request_id: u64,
        track_namespace: Vec<String>,
        error_code: u64,
        reason_phrase: String,
    ) -> Self {
        PublishNamespaceError {
            request_id,
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

impl MOQTMessage for PublishNamespaceError {
    fn depacketize(buf: &mut BytesMut) -> Result<Self, MOQTMessageError> {
        validate_header(ControlMessageType::PublishNamespaceError as u8, buf)?;

        let request_id = match read_variable_integer_from_buffer(buf) {
            Ok(v) => v,
            Err(_) => return Err(MOQTMessageError::ProtocolViolation),
        };

        let track_namespace_tuple_length = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("track namespace length")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let mut track_namespace_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let track_namespace = String::from_utf8(
                read_variable_bytes_from_buffer(buf)
                    .map_err(|_| MOQTMessageError::ProtocolViolation)?,
            )
            .context("track namespace")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
            track_namespace_tuple.push(track_namespace);
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

        Ok(PublishNamespaceError {
            request_id,
            track_namespace: track_namespace_tuple,
            error_code,
            reason_phrase,
        })
    }

    fn packetize(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.extend(write_variable_integer(self.request_id));
        let track_namespace_tuple_length = self.track_namespace.len() as u64;
        payload.extend(write_variable_integer(track_namespace_tuple_length));
        for track_namespace in &self.track_namespace {
            payload.extend(write_variable_bytes(&track_namespace.as_bytes().to_vec()));
        }
        payload.extend(write_variable_integer(self.error_code));
        payload.extend(write_variable_bytes(
            &self.reason_phrase.as_bytes().to_vec(),
        ));

        add_header(ControlMessageType::PublishNamespaceError as u8, payload)
    }
    /// Method to enable downcasting from MOQTPayload to AnnounceError
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::messages::{
            control_messages::publish_namespace_error::PublishNamespaceError,
            moqt_message::MOQTMessage,
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let request_id = 0;
            let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
            let error_code = 1;
            let reason_phrase = "already exist".to_string();

            let announce_error = PublishNamespaceError::new(
                request_id,
                track_namespace.clone(),
                error_code,
                reason_phrase.clone(),
            );
            let buf = announce_error.packetize();
            let expected_bytes_array = [
                8,  // Message Type(i)
                27, // Message Length(i)
                0,  // Request ID(i)
                2,  // Track Namespace(tuple): Number of elements
                4,  // Track Namespace(b): Length
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
        fn depacketize() {
            let bytes_array = [
                8,  // Message Type(i)
                27, // Message Length(i)
                0,  // Request ID(i)
                2,  // Track Namespace(tuple): Number of elements
                4,  // Track Namespace(b): Length
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
            let depacketized_announce_error = PublishNamespaceError::depacketize(&mut buf).unwrap();

            let request_id = 0;
            let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
            let error_code: u64 = 1;
            let reason_phrase = "already exist".to_string();
            let expected_announce_error = PublishNamespaceError::new(
                request_id,
                track_namespace.clone(),
                error_code,
                reason_phrase.clone(),
            );

            assert_eq!(depacketized_announce_error, expected_announce_error);
        }
    }
}
