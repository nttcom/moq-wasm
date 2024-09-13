use anyhow::{Context, Result};
use serde::Serialize;
use std::any::Any;

use super::moqt_payload::MOQTPayload;
use crate::{
    modules::variable_bytes::write_variable_bytes, variable_bytes::read_variable_bytes_from_buffer,
};

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct AnnounceOk {
    track_namespace: String,
}

impl AnnounceOk {
    pub fn new(track_namespace: String) -> Self {
        Self { track_namespace }
    }
}

impl MOQTPayload for AnnounceOk {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let track_namespace =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("track namespace")?;

        tracing::trace!("Depacketized Announce OK message.");

        Ok(AnnounceOk { track_namespace })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        // Track Namespace
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));

        tracing::trace!("Packetized Announce OK message.");
    }
    /// Method to enable downcasting from MOQTPayload to AnnounceOk
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::messages::moqt_payload::MOQTPayload;
    use crate::modules::messages::announce_ok::AnnounceOk;
    use bytes::BytesMut;

    #[test]
    fn packetize() {
        let track_namespace = "test".to_string();
        let announce_ok = AnnounceOk::new(track_namespace.clone());
        let mut buf = BytesMut::new();
        announce_ok.packetize(&mut buf);

        let expected_bytes_array = [
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
        ];
        assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
    }

    #[test]
    fn depacketize() {
        let bytes_array = [
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let announce_ok = AnnounceOk::depacketize(&mut buf).unwrap();

        let expected_announce_ok = AnnounceOk::new("test".to_string());
        assert_eq!(announce_ok, expected_announce_ok);
    }
}
