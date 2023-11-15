use anyhow::Result;

use crate::modules::{
    variable_bytes::read_variable_bytes_from_buffer,
    variable_integer::read_variable_integer_from_buffer,
};

use super::{payload::Payload, version_specific_parameters::TrackRequestParameter};

pub(crate) struct AnnounceMessage {
    track_namespace: String,
    number_of_parameters: u8,
    parameters: Vec<TrackRequestParameter>,
}

impl AnnounceMessage {
    pub(crate) fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
}

impl Payload for AnnounceMessage {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let track_namespace = read_variable_bytes_from_buffer(buf)?;

        let number_of_parameters = u8::try_from(read_variable_integer_from_buffer(buf)?)?;

        let mut parameters = vec![];
        for _ in 0..number_of_parameters {
            let param = TrackRequestParameter::depacketize(buf)?;

            if let TrackRequestParameter::Unknown(code) = param {
                tracing::info!("Unknown parameter: {}", code);
            } else {
                parameters.push(param);
            }
        }

        let announce_message = AnnounceMessage {
            track_namespace: String::from_utf8(track_namespace)?,
            number_of_parameters,
            parameters,
        };

        Ok(announce_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        todo!()
    }
}
