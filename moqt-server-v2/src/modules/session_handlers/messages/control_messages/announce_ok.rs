use crate::{
    modules::session_handlers::messages::moqt_payload::MOQTPayload,
    modules::session_handlers::messages::variable_bytes::{
        read_variable_bytes_from_buffer, write_variable_bytes,
    },
    modules::session_handlers::messages::variable_integer::{
        read_variable_integer_from_buffer, write_variable_integer,
    },
};
use anyhow::{Context, Result};
use bytes::BytesMut;
use serde::Serialize;
use std::any::Any;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct AnnounceOk {
    track_namespace: Vec<String>,
}

impl AnnounceOk {
    pub fn new(track_namespace: Vec<String>) -> Self {
        Self { track_namespace }
    }

    pub fn track_namespace(&self) -> &Vec<String> {
        &self.track_namespace
    }
}

impl MOQTPayload for AnnounceOk {
    fn depacketize(buf: &mut BytesMut) -> Result<Self> {
        let track_namespace_tuple_length = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("track namespace length")?;
        let mut track_namespace_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let track_namespace = String::from_utf8(read_variable_bytes_from_buffer(buf)?)
                .context("track namespace")?;
            track_namespace_tuple.push(track_namespace);
        }
        Ok(AnnounceOk {
            track_namespace: track_namespace_tuple,
        })
    }

    fn packetize(&self, buf: &mut BytesMut) {
        let track_namespace_tuple_length = self.track_namespace.len();
        buf.extend(write_variable_integer(track_namespace_tuple_length as u64));
        for track_namespace in &self.track_namespace {
            buf.extend(write_variable_bytes(&track_namespace.as_bytes().to_vec()));
        }
    }
    /// Method to enable downcasting from MOQTPayload to AnnounceOk
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::session_handlers::messages::{
            control_messages::announce_ok::AnnounceOk, moqt_payload::MOQTPayload,
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
            let announce_ok = AnnounceOk::new(track_namespace.clone());
            let mut buf = BytesMut::new();
            announce_ok.packetize(&mut buf);

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
            let announce_ok = AnnounceOk::depacketize(&mut buf).unwrap();

            let expected_announce_ok =
                AnnounceOk::new(Vec::from(["test".to_string(), "test".to_string()]));
            assert_eq!(announce_ok, expected_announce_ok);
        }
    }
}
