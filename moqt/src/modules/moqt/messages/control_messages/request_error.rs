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
pub struct RequestError {
    pub(crate) request_id: u64,
    pub(crate) error_code: u64,
    pub(crate) reason_phrase: String,
}

impl MOQTMessage for RequestError {
    fn depacketize(buf: &mut BytesMut) -> Result<Self, MOQTMessageError> {
        if !validate_payload_length(buf) {
            return Err(MOQTMessageError::ProtocolViolation);
        }

        let request_id = match read_variable_integer_from_buffer(buf) {
            Ok(v) => v,
            Err(_) => return Err(MOQTMessageError::ProtocolViolation),
        };

        let error_code = read_variable_integer_from_buffer(buf)
            .context("error code")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let reason_phrase = String::from_utf8(
            read_variable_bytes_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("reason phrase")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;

        Ok(RequestError {
            request_id,
            error_code,
            reason_phrase,
        })
    }

    fn packetize(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.extend(write_variable_integer(self.request_id));
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
            control_messages::request_error::RequestError, moqt_message::MOQTMessage,
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let request_id = 0;
            let error_code = 1;
            let reason_phrase = "already exist".to_string();

            let announce_error = RequestError {
                request_id,
                error_code,
                reason_phrase: reason_phrase.clone(),
            };
            let buf = announce_error.packetize();
            let expected_bytes_array = [
                16, // Message Length(i)
                0,  // Request ID(i)
                1,  // Error Code (i)
                13, // Reason Phrase (b): length
                97, 108, 114, 101, 97, 100, 121, 32, 101, 120, 105, 115,
                116, // Reason Phrase (b): Value("already exist")
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize() {
            let bytes_array = [
                16, // Message Length(i)
                0,  // Request ID(i)
                1,  // Error Code (i)
                13, // Reason Phrase (b): length
                97, 108, 114, 101, 97, 100, 121, 32, 101, 120, 105, 115,
                116, // Reason Phrase (b): Value("already exist")
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let depacketized_announce_error = RequestError::depacketize(&mut buf).unwrap();

            let request_id = 0;
            let error_code: u64 = 1;
            let reason_phrase = "already exist".to_string();
            let expected_announce_error = RequestError {
                request_id,
                error_code,
                reason_phrase: reason_phrase.clone(),
            };

            assert_eq!(depacketized_announce_error, expected_announce_error);
        }
    }
}
