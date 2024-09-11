use super::moqt_payload::MOQTPayload;
use crate::{
    modules::variable_bytes::read_variable_bytes_from_buffer, variable_bytes::write_variable_bytes,
};
use anyhow::{Context, Result};
use std::any::Any;

#[derive(Debug, Clone, PartialEq)]
pub struct UnAnnounce {
    track_namespace: String,
}

impl UnAnnounce {
    pub fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
}

impl MOQTPayload for UnAnnounce {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let track_namespace = read_variable_bytes_from_buffer(buf).context("track namespace")?;

        let unannounce_message = UnAnnounce {
            track_namespace: String::from_utf8(track_namespace)?,
        };

        tracing::trace!("Depacketized Unannounce message.");

        Ok(unannounce_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));

        tracing::trace!("Packetized Unannounce message.");
    }
    /// Method to enable downcasting from MOQTPayload to UnAnnounce
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::messages::moqt_payload::MOQTPayload;
    use crate::messages::unannounce::UnAnnounce;
    use bytes::BytesMut;

    #[test]
    fn packetize_unannounce() {
        let unannounce = UnAnnounce {
            track_namespace: "track_namespace".to_string(),
        };

        let mut buf = BytesMut::new();
        unannounce.packetize(&mut buf);

        let expected_bytes_array = [
            15, // track_namespace length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, 115, 112, 97, 99,
            101, // track_namespace bytes("track_namespace")
        ];
        assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
    }

    #[test]
    fn depacketize_unannounce() {
        let bytes_array = [
            15, // track_namespace length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, 115, 112, 97, 99,
            101, // track_namespace bytes("track_namespace")
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_unannounce = UnAnnounce::depacketize(&mut buf).unwrap();

        let expected_unannounce = UnAnnounce {
            track_namespace: "track_namespace".to_string(),
        };

        assert_eq!(depacketized_unannounce, expected_unannounce);
    }
}
