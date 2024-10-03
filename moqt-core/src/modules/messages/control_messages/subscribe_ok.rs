use anyhow::Context;
use serde::Serialize;
use std::any::Any;

use crate::messages::moqt_payload::MOQTPayload;
use crate::modules::{
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SubscribeOk {
    track_namespace: Vec<String>,
    track_name: String,
    track_id: u64,
    expires: u64,
}

impl SubscribeOk {
    pub fn new(
        track_namespace: Vec<String>,
        track_name: String,
        track_id: u64,
        expires: u64,
    ) -> SubscribeOk {
        SubscribeOk {
            track_namespace,
            track_name,
            track_id,
            expires,
        }
    }

    pub fn track_namespace(&self) -> &Vec<String> {
        &self.track_namespace
    }
    pub fn track_name(&self) -> &str {
        &self.track_name
    }
    pub fn track_id(&self) -> u64 {
        self.track_id
    }
}

impl MOQTPayload for SubscribeOk {
    fn depacketize(buf: &mut bytes::BytesMut) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let track_namespace_tuple_length = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("track namespace length")?;
        let mut track_namespace_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let track_namespace = String::from_utf8(read_variable_bytes_from_buffer(buf)?)
                .context("track namespace")?;
            track_namespace_tuple.push(track_namespace);
        }
        let track_name =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("track name")?;
        let track_id = read_variable_integer_from_buffer(buf).context("track id")?;
        let expires = read_variable_integer_from_buffer(buf).context("expires")?;

        tracing::trace!("Depacketized Subscribe OK message.");

        Ok(SubscribeOk {
            track_namespace: track_namespace_tuple,
            track_name,
            track_id,
            expires,
        })
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
        buf.extend(write_variable_integer(self.track_id));
        buf.extend(write_variable_integer(self.expires));

        tracing::trace!("Packetized Subscribe OK message.");
    }
    /// Method to enable downcasting from MOQTPayload to SubscribeOk
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::{
        messages::moqt_payload::MOQTPayload,
        modules::messages::control_messages::subscribe_ok::SubscribeOk,
    };
    use bytes::BytesMut;

    #[test]
    fn packetize_subscribe_ok() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let track_id = 1;
        let expires = 2;
        let subscribe_ok = SubscribeOk::new(
            track_namespace.clone(),
            track_name.clone(),
            track_id,
            expires,
        );
        let mut buf = bytes::BytesMut::new();
        subscribe_ok.packetize(&mut buf);

        let expected_bytes_array = [
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
            1,   // Track ID (i)
            2,   // Expires (i)
        ];
        assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
    }

    #[test]
    fn depacketize_subscribe_ok() {
        let bytes_array = [
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
            1,   // Track ID (i)
            2,   // Expires (i)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_subscribe_ok = SubscribeOk::depacketize(&mut buf).unwrap();

        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let track_id = 1;
        let expires = 2;
        let expected_subscribe_ok = SubscribeOk::new(
            track_namespace.clone(),
            track_name.clone(),
            track_id,
            expires,
        );

        assert_eq!(depacketized_subscribe_ok, expected_subscribe_ok);
    }
}
