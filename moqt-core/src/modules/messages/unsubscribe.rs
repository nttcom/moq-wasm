use super::moqt_payload::MOQTPayload;
use crate::{
    modules::variable_bytes::read_variable_bytes_from_buffer, variable_bytes::write_variable_bytes,
};
use anyhow::{Context, Result};
use std::any::Any;

#[derive(Debug, Clone, PartialEq)]
pub struct Unsubscribe {
    track_namespace: String,
    track_name: String,
}

impl Unsubscribe {
    pub fn new(track_namespace: String, track_name: String) -> Unsubscribe {
        Unsubscribe {
            track_namespace,
            track_name,
        }
    }

    // TODO: Not implemented yet
    #[allow(dead_code)]
    pub(crate) fn track_namespace(&self) -> &str {
        &self.track_namespace
    }

    // TODO: Not implemented yet
    #[allow(dead_code)]
    pub(crate) fn track_name(&self) -> &str {
        &self.track_name
    }
}

impl MOQTPayload for Unsubscribe {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let track_namespace = read_variable_bytes_from_buffer(buf).context("track namespace")?;
        let track_name = read_variable_bytes_from_buffer(buf).context("track name")?;

        let unsubscribe_message = Unsubscribe {
            track_namespace: String::from_utf8(track_namespace)?,
            track_name: String::from_utf8(track_name)?,
        };

        tracing::trace!("Depacketized Unsubscribe message.");

        Ok(unsubscribe_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));
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
    use crate::messages::moqt_payload::MOQTPayload;
    use crate::messages::unsubscribe::Unsubscribe;
    use bytes::BytesMut;
    #[test]
    fn packetize_unsubscribe() {
        let unsubscribe = Unsubscribe {
            track_namespace: "track_namespace".to_string(),
            track_name: "track_name".to_string(),
        };

        let mut buf = BytesMut::new();
        unsubscribe.packetize(&mut buf);

        let expected_bytes_array = [
            15, // track_namespace length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, 115, 112, 97, 99,
            101, // track_namespace bytes("track_namespace")
            10,  // track_name length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, // track_name bytes("track_name")
        ];
        assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
    }
    #[test]
    fn depacketize_unsubscribe() {
        let bytes_array = [
            15, // track_namespace length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, 115, 112, 97, 99,
            101, // track_namespace bytes("track_namespace")
            10,  // track_name length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, // track_name bytes("track_name")
        ];
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&bytes_array);
        let depacketized_unsubscribe = Unsubscribe::depacketize(&mut buf).unwrap();

        let expected_unsubscribe = Unsubscribe {
            track_namespace: "track_namespace".to_string(),
            track_name: "track_name".to_string(),
        };

        assert_eq!(depacketized_unsubscribe, expected_unsubscribe);
    }
}
