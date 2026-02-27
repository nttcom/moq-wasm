use anyhow::{Context, Result};
use bytes::BytesMut;
use std::any::Any;

use crate::modules::moqt::control_plane::control_messages::{
    moqt_payload::MOQTPayload,
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

#[derive(Debug, Clone, PartialEq)]
pub struct UnAnnounce {
    track_namespace: Vec<String>,
}

impl UnAnnounce {
    pub fn new(track_namespace: Vec<String>) -> Self {
        UnAnnounce { track_namespace }
    }
    pub fn track_namespace(&self) -> &Vec<String> {
        &self.track_namespace
    }
}

impl MOQTPayload for UnAnnounce {
    fn depacketize(buf: &mut BytesMut) -> Result<Self> {
        let track_namespace_tuple_length = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("track namespace length")?;
        let mut track_namespace_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let track_namespace = String::from_utf8(read_variable_bytes_from_buffer(buf)?)
                .context("track namespace")?;
            track_namespace_tuple.push(track_namespace);
        }

        let unannounce_message = UnAnnounce {
            track_namespace: track_namespace_tuple,
        };

        Ok(unannounce_message)
    }

    fn packetize(&self, buf: &mut BytesMut) {
        // Track Namespace Number of elements
        let track_namespace_tuple_length = self.track_namespace.len();
        buf.extend(write_variable_integer(track_namespace_tuple_length as u64));
        for track_namespace in &self.track_namespace {
            // Track Namespace
            buf.extend(write_variable_bytes(&track_namespace.as_bytes().to_vec()));
        }
    }
    /// Method to enable downcasting from MOQTPayload to UnAnnounce
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::control_plane::control_messages::{
            messages::unannounce::UnAnnounce, moqt_payload::MOQTPayload,
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let unannounce = UnAnnounce {
                track_namespace: Vec::from(["test".to_string(), "test".to_string()]),
            };

            let mut buf = BytesMut::new();
            unannounce.packetize(&mut buf);

            let expected_bytes_array = [
                2, // Track Namespace(tuple): Number of elements
                4, // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                4,   // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize() {
            let bytes_array = [
                2, // Track Namespace(tuple): Number of elements
                4, // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                4,   // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let depacketized_unannounce = UnAnnounce::depacketize(&mut buf).unwrap();

            let expected_unannounce = UnAnnounce {
                track_namespace: Vec::from(["test".to_string(), "test".to_string()]),
            };

            assert_eq!(depacketized_unannounce, expected_unannounce);
        }
    }
}
