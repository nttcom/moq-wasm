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
pub struct SubscribeNamespaceOk {
    pub(crate) request_id: u64,
    track_namespace_prefix: Vec<String>,
}

impl SubscribeNamespaceOk {
    pub fn new(request_id: u64, track_namespace_prefix: Vec<String>) -> Self {
        SubscribeNamespaceOk {
            request_id,
            track_namespace_prefix,
        }
    }

    pub fn track_namespace_prefix(&self) -> &Vec<String> {
        &self.track_namespace_prefix
    }
}

impl MOQTMessage for SubscribeNamespaceOk {
    fn depacketize(buf: &mut BytesMut) -> Result<Self, MOQTMessageError> {
        validate_header(ControlMessageType::SubscribeNamespaceOk as u8, buf)?;

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

        Ok(SubscribeNamespaceOk {
            request_id,
            track_namespace_prefix: track_namespace_prefix_tuple,
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
        add_header(ControlMessageType::SubscribeNamespaceOk as u8, payload)
    }
    /// Method to enable downcasting from MOQTPayload to SubscribeAnnouncesOk
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::messages::{
            control_messages::subscribe_namespace_ok::SubscribeNamespaceOk,
            moqt_message::MOQTMessage,
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let request_id = 0;
            let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
            let subscribe_announces_ok =
                SubscribeNamespaceOk::new(request_id, track_namespace_prefix.clone());
            let buf = subscribe_announces_ok.packetize();

            let expected_bytes_array = [
                18, // Message Type(i)
                12, // Message Length(i)
                0,  // Request ID(i)
                2,  // Track Namespace Prefix(tuple): Number of elements
                4,  // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                4,   // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize() {
            let bytes_array = [
                18, // Message Type(i)
                12, // Message Length(i)
                0,  // Request ID(i)
                2,  // Track Namespace Prefix(tuple): Number of elements
                4,  // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                4,   // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let subscribe_announces_ok = SubscribeNamespaceOk::depacketize(&mut buf).unwrap();

            let request_id = 0;
            let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
            let expected_subscribe_announces_ok =
                SubscribeNamespaceOk::new(request_id, track_namespace_prefix);

            assert_eq!(subscribe_announces_ok, expected_subscribe_announces_ok);
        }
    }
}
