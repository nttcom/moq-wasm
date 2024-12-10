use crate::{
    messages::moqt_payload::MOQTPayload,
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
use anyhow::{Context, Result};
use bytes::BytesMut;
use serde::Serialize;
use std::any::Any;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SubscribeNamespaceOk {
    track_namespace_prefix: Vec<String>,
}

impl SubscribeNamespaceOk {
    pub fn new(track_namespace_prefix: Vec<String>) -> Self {
        SubscribeNamespaceOk {
            track_namespace_prefix,
        }
    }

    pub fn track_namespace_prefix(&self) -> &Vec<String> {
        &self.track_namespace_prefix
    }
}

impl MOQTPayload for SubscribeNamespaceOk {
    fn depacketize(buf: &mut BytesMut) -> Result<Self> {
        let track_namespace_prefix_tuple_length =
            u8::try_from(read_variable_integer_from_buffer(buf)?)
                .context("track namespace prefix length")?;
        let mut track_namespace_prefix_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_prefix_tuple_length {
            let track_namespace_prefix = String::from_utf8(read_variable_bytes_from_buffer(buf)?)
                .context("track namespace prefix")?;
            track_namespace_prefix_tuple.push(track_namespace_prefix);
        }

        Ok(SubscribeNamespaceOk {
            track_namespace_prefix: track_namespace_prefix_tuple,
        })
    }

    fn packetize(&self, buf: &mut BytesMut) {
        let track_namespace_prefix_tuple_length = self.track_namespace_prefix.len();
        buf.extend(write_variable_integer(
            track_namespace_prefix_tuple_length as u64,
        ));
        for track_namespace_prefix in &self.track_namespace_prefix {
            buf.extend(write_variable_bytes(
                &track_namespace_prefix.as_bytes().to_vec(),
            ));
        }
    }
    /// Method to enable downcasting from MOQTPayload to SubscribeNamespaceOk
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::messages::{
            control_messages::subscribe_namespace_ok::SubscribeNamespaceOk,
            moqt_payload::MOQTPayload,
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
            let subscribe_namespace_ok = SubscribeNamespaceOk::new(track_namespace_prefix.clone());
            let mut buf = BytesMut::new();
            subscribe_namespace_ok.packetize(&mut buf);

            let expected_bytes_array = [
                2, // Track Namespace Prefix(tuple): Number of elements
                4, // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                4,   // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize() {
            let bytes_array = [
                2, // Track Namespace Prefix(tuple): Number of elements
                4, // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                4,   // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let subscribe_namespace_ok = SubscribeNamespaceOk::depacketize(&mut buf).unwrap();

            let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
            let expected_subscribe_namespace_ok = SubscribeNamespaceOk::new(track_namespace_prefix);

            assert_eq!(subscribe_namespace_ok, expected_subscribe_namespace_ok);
        }
    }
}
