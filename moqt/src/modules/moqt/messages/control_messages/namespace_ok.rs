use crate::modules::moqt::messages::{
    control_messages::util::{add_payload_length, validate_payload_length},
    moqt_message::MOQTMessage,
    moqt_message_error::MOQTMessageError,
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
use anyhow::Result;
use bytes::BytesMut;
use serde::Serialize;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct NamespaceOk {
    pub(crate) request_id: u64,
}

impl MOQTMessage for NamespaceOk {
    fn depacketize(buf: &mut BytesMut) -> Result<Self, MOQTMessageError> {
        if !validate_payload_length(buf) {
            return Err(MOQTMessageError::ProtocolViolation);
        }

        let request_id = match read_variable_integer_from_buffer(buf) {
            Ok(v) => v,
            Err(_) => return Err(MOQTMessageError::ProtocolViolation),
        };
        Ok(NamespaceOk { request_id })
    }

    fn packetize(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.extend(write_variable_integer(self.request_id));

        add_payload_length(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::messages::{
            control_messages::namespace_ok::NamespaceOk, moqt_message::MOQTMessage,
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let request_id = 0;
            let announce_ok = NamespaceOk { request_id };
            let buf = announce_ok.packetize();

            let expected_bytes_array = [
                1, // Message Length(i)
                0, // Request ID(i)
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize() {
            let request_id = 0;
            let bytes_array = [
                1, // Message Length(i)
                0, // Request ID(i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let announce_ok = NamespaceOk::depacketize(&mut buf).unwrap();

            let expected_announce_ok = NamespaceOk { request_id };
            assert_eq!(announce_ok, expected_announce_ok);
        }
    }
}
