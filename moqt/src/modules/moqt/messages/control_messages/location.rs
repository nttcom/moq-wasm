use bytes::BytesMut;

use crate::modules::moqt::messages::variable_integer::read_variable_integer_from_buffer;
use crate::modules::moqt::messages::{
    moqt_message::MOQTMessage, moqt_message_error::MOQTMessageError,
    variable_integer::write_variable_integer,
};

#[derive(Debug, PartialEq)]
pub(super) struct Location {
    pub(super) group_id: u64,
    pub(super) object_id: u64,
}

impl MOQTMessage for Location {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self, MOQTMessageError> {
        let group_id = read_variable_integer_from_buffer(buf)
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let object_id = read_variable_integer_from_buffer(buf)
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        Ok(Self {
            group_id,
            object_id,
        })
    }

    fn packetize(&self) -> bytes::BytesMut {
        let mut payload = BytesMut::new();
        payload.extend(write_variable_integer(self.group_id));
        payload.extend(write_variable_integer(self.object_id));
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use bytes::BytesMut;

        use crate::modules::moqt::messages::{
            control_messages::location::Location, moqt_message::MOQTMessage,
        };

        #[test]
        fn packetize_and_depacketize() {
            let location = Location {
                group_id: 10,
                object_id: 5,
            };

            let mut buf = location.packetize();

            // depacketize
            let depacketized_location = Location::depacketize(&mut buf).unwrap();

            assert_eq!(location.group_id, depacketized_location.group_id);
            assert_eq!(location.object_id, depacketized_location.object_id);
        }

        #[test]
        fn packetize_check_bytes() {
            let location = Location {
                group_id: 10,
                object_id: 5,
            };

            let mut buf = BytesMut::new();
            buf.extend(location.packetize());

            let expected_bytes = vec![10, 5];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }
    }
}
