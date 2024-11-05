use crate::messages::moqt_payload::MOQTPayload;
use crate::variable_integer::{read_variable_integer_from_buffer, write_variable_integer};
use crate::{
    modules::variable_bytes::read_variable_bytes_from_buffer, variable_bytes::write_variable_bytes,
};
use anyhow::{Context, Result};
use std::any::Any;

#[derive(Debug, Clone, PartialEq)]
pub struct Unsubscribe {
    track_namespace: Vec<String>,
    track_name: String,
}

impl Unsubscribe {
    pub fn new(track_namespace: Vec<String>, track_name: String) -> Unsubscribe {
        Unsubscribe {
            track_namespace,
            track_name,
        }
    }
}

impl MOQTPayload for Unsubscribe {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let track_namespace_tuple_length = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("track namespace length")?;
        let mut track_namespace_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let track_namespace = String::from_utf8(read_variable_bytes_from_buffer(buf)?)
                .context("track namespace")?;
            track_namespace_tuple.push(track_namespace);
        }
        let track_name = read_variable_bytes_from_buffer(buf).context("track name")?;

        let unsubscribe_message = Unsubscribe {
            track_namespace: track_namespace_tuple,
            track_name: String::from_utf8(track_name)?,
        };

        Ok(unsubscribe_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        // Track Namespace Number of elements
        let track_namespace_tuple_length = self.track_namespace.len();
        buf.extend(write_variable_integer(track_namespace_tuple_length as u64));
        for track_namespace in &self.track_namespace {
            // Track Namespace
            buf.extend(write_variable_bytes(&track_namespace.as_bytes().to_vec()));
        }
        buf.extend(write_variable_bytes(&self.track_name.as_bytes().to_vec()));

        tracing::trace!("Packetized Unsubscribe message.");
    }
    /// Method to enable downcasting from MOQTPayload to Unsubscribe
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::messages::control_messages::unsubscribe::Unsubscribe;
    use crate::messages::moqt_payload::MOQTPayload;
    use bytes::BytesMut;
    #[test]
    fn packetize_unsubscribe() {
        let unsubscribe = Unsubscribe {
            track_namespace: Vec::from(["test".to_string(), "test".to_string()]),
            track_name: "track_name".to_string(),
        };

        let mut buf = BytesMut::new();
        unsubscribe.packetize(&mut buf);

        let expected_bytes_array = [
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
        ];
        assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
    }
    #[test]
    fn depacketize_unsubscribe() {
        let bytes_array = [
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
        ];
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&bytes_array);
        let depacketized_unsubscribe = Unsubscribe::depacketize(&mut buf).unwrap();

        let expected_unsubscribe = Unsubscribe {
            track_namespace: Vec::from(["test".to_string(), "test".to_string()]),
            track_name: "track_name".to_string(),
        };

        assert_eq!(depacketized_unsubscribe, expected_unsubscribe);
    }
}
