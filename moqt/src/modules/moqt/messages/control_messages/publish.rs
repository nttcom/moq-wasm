use anyhow::Context;
use bytes::BytesMut;

use crate::modules::moqt::messages::{
    control_messages::{
        location::Location,
        util::{add_payload_length, validate_payload_length},
        version_specific_parameters::VersionSpecificParameter,
    },
    moqt_message::MOQTMessage,
    moqt_message_error::MOQTMessageError,
    moqt_payload::MOQTPayload,
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

pub(crate) struct Publish {
    pub(super) request_id: u64,
    pub(super) track_namespace_tuple: Vec<String>,
    pub(super) track_name: String,
    pub(super) track_alias: u64,
    pub(super) group_order: u8,
    pub(super) content_exists: u8,
    pub(super) largest_location: Option<Location>,
    pub(super) forward: u8,
    pub(super) number_of_parameters: u8,
    pub(super) parameters: Vec<VersionSpecificParameter>,
}

impl MOQTMessage for Publish {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self, MOQTMessageError> {
        if !validate_payload_length(buf) {
            return Err(MOQTMessageError::ProtocolViolation);
        }
        let request_id = match read_variable_integer_from_buffer(buf) {
            Ok(v) => v,
            Err(_) => return Err(MOQTMessageError::ProtocolViolation),
        };
        let track_namespace_tuple_length = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("track namespace length")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let mut track_namespace_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let track_namespace = String::from_utf8(
                read_variable_bytes_from_buffer(buf)
                    .map_err(|_| MOQTMessageError::ProtocolViolation)?,
            )
            .context("track namespace")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
            track_namespace_tuple.push(track_namespace);
        }
        let track_name = String::from_utf8(
            read_variable_bytes_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let track_alias = match read_variable_integer_from_buffer(buf) {
            Ok(v) => v,
            Err(_) => return Err(MOQTMessageError::ProtocolViolation),
        };
        let group_order = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("track namespace length")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let content_exists = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("track namespace length")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        // location
        let largest_location = if content_exists == 1 {
            let group_id = match read_variable_integer_from_buffer(buf) {
                Ok(v) => v,
                Err(_) => return Err(MOQTMessageError::ProtocolViolation),
            };
            let object_id = match read_variable_integer_from_buffer(buf) {
                Ok(v) => v,
                Err(_) => return Err(MOQTMessageError::ProtocolViolation),
            };
            Some(Location {
                group_id,
                object_id,
            })
        } else {
            None
        };

        let forward = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("track namespace length")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let number_of_parameters = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("number of parameters")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let mut parameters = vec![];
        for _ in 0..number_of_parameters {
            let version_specific_parameter = VersionSpecificParameter::depacketize(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?;
            if let VersionSpecificParameter::Unknown(code) = version_specific_parameter {
                tracing::warn!("unknown track request parameter {}", code);
            } else {
                parameters.push(version_specific_parameter);
            }
        }
        Ok(Self {
            request_id,
            track_namespace_tuple,
            track_name,
            track_alias,
            group_order,
            content_exists,
            largest_location,
            forward,
            number_of_parameters,
            parameters,
        })
    }

    fn packetize(&self) -> bytes::BytesMut {
        let mut payload = BytesMut::new();
        payload.extend(write_variable_integer(self.request_id));
        let track_namespace_tuple_length = self.track_namespace_tuple.len();
        payload.extend(write_variable_integer(track_namespace_tuple_length as u64));
        for track_namespace in &self.track_namespace_tuple {
            payload.extend(write_variable_bytes(&track_namespace.as_bytes().to_vec()));
        }
        let track_name_length = self.track_name.len();
        payload.extend(write_variable_integer(track_name_length as u64));
        payload.extend(write_variable_bytes(&self.track_name.as_bytes().to_vec()));

        payload.extend(write_variable_integer(self.track_alias));
        payload.extend(write_variable_integer(self.group_order as u64));

        payload.extend(write_variable_integer(self.content_exists as u64));
        // location
        if let Some(location) = &self.largest_location {
            payload.extend(write_variable_integer(location.group_id));
            payload.extend(write_variable_integer(location.object_id));
        }
        payload.extend(write_variable_integer(self.forward as u64));

        payload.extend(write_variable_integer(self.number_of_parameters as u64));
        // Parameters
        for param in &self.parameters {
            param.packetize(&mut payload);
        }

        tracing::trace!("Packetized Announce message.");
        add_payload_length(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {
        
    }
}
