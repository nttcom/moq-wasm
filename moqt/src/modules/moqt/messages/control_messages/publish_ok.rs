use anyhow::Context;
use bytes::BytesMut;

use crate::modules::moqt::messages::{
    control_messages::{
        enums::FilterType,
        location::Location,
        util::{add_payload_length, validate_payload_length},
        version_specific_parameters::VersionSpecificParameter,
    },
    moqt_message::MOQTMessage,
    moqt_message_error::MOQTMessageError,
    moqt_payload::MOQTPayload,
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

pub(super) struct PublishOk {
    pub(super) request_id: u64,
    pub(super) forward: u8,
    pub(super) subscriber_priority: u8,
    pub(super) group_order: u8,
    /**
     * filter type
     * LatestGroup(0x01) = start location
     * LatestObject(0x02) = start location
     * AbsoluteStart(0x03) = start location
     * AbsoluteRange(0x04) = start location && end group
     */
    pub(super) filter_type: FilterType,
    pub(super) start_location: Option<Location>,
    pub(super) end_group: Option<u64>,
    pub(super) number_of_parameters: u8,
    pub(super) parameters: Vec<VersionSpecificParameter>,
}

impl MOQTMessage for PublishOk {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self, MOQTMessageError> {
        if !validate_payload_length(buf) {
            return Err(MOQTMessageError::ProtocolViolation);
        }
        let request_id = match read_variable_integer_from_buffer(buf) {
            Ok(v) => v,
            Err(_) => return Err(MOQTMessageError::ProtocolViolation),
        };
        let forward = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("forward")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let subscriber_priority = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("subscriber priority")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let group_order = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("group order")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let filter_type = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("group order")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let filter_type =
            FilterType::try_from(filter_type).map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let (start_location, end_group) = match filter_type {
            FilterType::AbsoluteStart => (Some(Location::depacketize(buf)?), None),
            FilterType::AbsoluteRange => {
                let start_location = Location::depacketize(buf)?;
                let end_group = read_variable_integer_from_buffer(buf)
                    .map_err(|_| MOQTMessageError::ProtocolViolation)?;
                (Some(start_location), Some(end_group))
            }
            _ => {
                tracing::info!(
                    "Filter Type: {:?} has no start location, end group as well",
                    filter_type
                );
                (None, None)
            }
        };
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
            forward,
            subscriber_priority,
            group_order,
            filter_type,
            start_location,
            end_group,
            number_of_parameters,
            parameters,
        })
    }

    fn packetize(&self) -> bytes::BytesMut {
        let mut payload = BytesMut::new();
        payload.extend(write_variable_integer(self.request_id));
        payload.extend(write_variable_integer(self.forward as u64));
        payload.extend(write_variable_integer(self.subscriber_priority as u64));
        payload.extend(write_variable_integer(self.group_order as u64));
        payload.extend(write_variable_integer(self.filter_type as u64));
        match self.filter_type {
            FilterType::AbsoluteStart => {
                let bytes = self.start_location.as_ref().unwrap().packetize();
                payload.extend(bytes);
            }
            FilterType::AbsoluteRange => {
                let bytes = self.start_location.as_ref().unwrap().packetize();
                payload.extend(bytes);
                payload.extend(write_variable_integer(self.end_group.unwrap()));
            }
            _ => {
                tracing::info!(
                    "Filter Type: {:?} has no start location, end group as well",
                    self.filter_type
                );
            }
        };
        payload.extend(write_variable_integer(self.number_of_parameters as u64));
        // Parameters
        for param in &self.parameters {
            param.packetize(&mut payload);
        }

        tracing::trace!("Packetized Publish message.");
        add_payload_length(payload)
    }
}
