use crate::{
    modules::session_handlers::messages::moqt_payload::MOQTPayload,
    modules::session_handlers::messages::variable_integer::{
        read_variable_integer_from_buffer, write_variable_integer,
    },
};
use anyhow::{Context, Result};
use bytes::BytesMut;
use std::any::Any;

#[derive(Debug, Clone, PartialEq)]
pub struct Unsubscribe {
    subscribe_id: u64,
}

impl Unsubscribe {
    pub fn new(subscribe_id: u64) -> Unsubscribe {
        Unsubscribe { subscribe_id }
    }
    pub fn subscribe_id(&self) -> u64 {
        self.subscribe_id
    }
}

impl MOQTPayload for Unsubscribe {
    fn depacketize(buf: &mut BytesMut) -> Result<Self> {
        let subscribe_id = read_variable_integer_from_buffer(buf).context("subscribe id")?;
        let unsubscribe_message = Unsubscribe { subscribe_id };
        Ok(unsubscribe_message)
    }
    fn packetize(&self, buf: &mut BytesMut) {
        buf.extend(write_variable_integer(self.subscribe_id));
    }
    /// Method to enable downcasting from MOQTPayload to Unsubscribe
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::session_handlers::messages::{
            control_messages::unsubscribe::Unsubscribe, moqt_payload::MOQTPayload,
        };
        use bytes::BytesMut;
        #[test]
        fn packetize() {
            let unsubscribe = Unsubscribe::new(0);
            let mut buf = BytesMut::new();
            unsubscribe.packetize(&mut buf);

            let expected_bytes_array = [
                0, // Subscribe ID (i)
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }
        #[test]
        fn depacketize() {
            let bytes_array = [
                0, // Subscribe ID (i)
            ];
            let mut buf = BytesMut::new();
            buf.extend_from_slice(&bytes_array);
            let depacketized_unsubscribe = Unsubscribe::depacketize(&mut buf).unwrap();

            let expected_unsubscribe = Unsubscribe::new(0);

            assert_eq!(depacketized_unsubscribe, expected_unsubscribe);
        }
    }
}
