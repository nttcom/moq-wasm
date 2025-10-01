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
pub struct PublishNamespaceOk {
    pub(crate) request_id: u64,
    track_namespace: Vec<String>,
}

impl PublishNamespaceOk {
    pub fn new(request_id: u64, track_namespace: Vec<String>) -> Self {
        Self {
            request_id,
            track_namespace,
        }
    }

    pub fn track_namespace(&self) -> &Vec<String> {
        &self.track_namespace
    }
}

impl MOQTMessage for PublishNamespaceOk {
    fn depacketize(mut buf: &mut BytesMut) -> Result<Self, MOQTMessageError> {
        if !validate_payload_length(buf) {
            return Err(MOQTMessageError::ProtocolViolation);
        }

        let request_id = match read_variable_integer_from_buffer(&mut buf) {
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
        Ok(PublishNamespaceOk {
            request_id,
            track_namespace: track_namespace_tuple,
        })
    }

    fn packetize(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.extend(write_variable_integer(self.request_id));
        let track_namespace_tuple_length = self.track_namespace.len();
        payload.extend(write_variable_integer(track_namespace_tuple_length as u64));
        for track_namespace in &self.track_namespace {
            payload.extend(write_variable_bytes(&track_namespace.as_bytes().to_vec()));
        }

        add_payload_length(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::messages::{
            control_messages::publish_namespace_ok::PublishNamespaceOk, moqt_message::MOQTMessage,
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let request_id = 0;
            let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
            let announce_ok = PublishNamespaceOk::new(request_id, track_namespace.clone());
            let buf = announce_ok.packetize();

            let expected_bytes_array = [
                12, // Message Length(i)
                0,  // Request ID(i)
                2,  // Track Namespace(tuple): Number of elements
                4,  // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                4,   // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize() {
            let request_id = 0;
            let bytes_array = [
                12, // Message Length(i)
                0,  // Request ID(i)
                2,  // Track Namespace(tuple): Number of elements
                4,  // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                4,   // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let announce_ok = PublishNamespaceOk::depacketize(&mut buf).unwrap();

            let expected_announce_ok = PublishNamespaceOk::new(
                request_id,
                Vec::from(["test".to_string(), "test".to_string()]),
            );
            assert_eq!(announce_ok, expected_announce_ok);
        }
    }
}
