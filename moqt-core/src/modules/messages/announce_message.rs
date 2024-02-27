use anyhow::{Context, Result};

use crate::modules::{
    variable_bytes::read_variable_bytes_from_buffer,
    variable_integer::read_variable_integer_from_buffer,
};

use super::{moqt_payload::MOQTPayload, version_specific_parameters::TrackRequestParameter};

pub(crate) struct AnnounceMessage {
    track_namespace: String,
    _number_of_parameters: u8,               // TODO: 未実装
    _parameters: Vec<TrackRequestParameter>, // TODO: 未実装
}

impl AnnounceMessage {
    pub(crate) fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
}

impl MOQTPayload for AnnounceMessage {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let track_namespace = read_variable_bytes_from_buffer(buf).context("track namespace")?;

        let number_of_parameters = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("number of parameters")?;

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
            _number_of_parameters: number_of_parameters, // TODO: 未実装のため_をつけている
            _parameters: parameters,                     // TODO: 未実装のため_をつけている
        };

        Ok(announce_message)
    }

    // TODO: 未実装のため_をつけている
    fn packetize(&self, _buf: &mut bytes::BytesMut) {
        todo!()
    }
}
