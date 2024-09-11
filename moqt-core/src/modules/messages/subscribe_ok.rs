use anyhow::Context;
use serde::Serialize;
use std::any::Any;

use crate::modules::{
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

use super::moqt_payload::MOQTPayload;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SubscribeOk {
    track_namespace: String,
    track_name: String,
    track_id: u64,
    expires: u64,
}

impl SubscribeOk {
    pub fn new(
        track_namespace: String,
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

    pub fn track_namespace(&self) -> &str {
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
        let full_track_namespace =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("track namespace")?;
        let full_track_name =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("track name")?;
        let track_id = read_variable_integer_from_buffer(buf).context("track id")?;
        let expires = read_variable_integer_from_buffer(buf).context("expires")?;

        Ok(SubscribeOk {
            track_namespace: full_track_namespace,
            track_name: full_track_name,
            track_id,
            expires,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));
        buf.extend(write_variable_bytes(&self.track_name.as_bytes().to_vec()));
        buf.extend(write_variable_integer(self.track_id));
        buf.extend(write_variable_integer(self.expires));
    }
    /// Method to enable downcasting from MOQTPayload to SubscribeOk
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::{
        messages::moqt_payload::MOQTPayload, modules::messages::subscribe_ok::SubscribeOk,
    };
    use bytes::BytesMut;

    #[test]
    fn packetize_subscribe_ok() {
        let track_namespace = "track_namespace".to_string();
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
            15, // track_namespace length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, 115, 112, 97, 99,
            101, // track_namespace bytes("track_namespace")
            10,  // track_name length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, // track_name bytes("track_name")
            1,   // track_id
            2,   // expires
        ];
        assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
    }

    #[test]
    fn depacketize_subscribe_ok() {
        let bytes_array = [
            15, // track_namespace length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, 115, 112, 97, 99,
            101, // track_namespace bytes("track_namespace")
            10,  // track_name length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, // track_name bytes("track_name")
            1,   // track_id
            2,   // expires
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_subscribe_ok = SubscribeOk::depacketize(&mut buf).unwrap();

        let track_namespace = "track_namespace".to_string();
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
