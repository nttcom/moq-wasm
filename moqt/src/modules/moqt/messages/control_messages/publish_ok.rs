use anyhow::Context;
use bytes::BytesMut;

use crate::modules::moqt::messages::{
    control_messages::{
        enums::FilterType,
        group_order::GroupOrder,
        location::Location,
        util::{self, add_payload_length, validate_payload_length},
        version_specific_parameters::VersionSpecificParameter,
    },
    moqt_message::MOQTMessage,
    moqt_message_error::MOQTMessageError,
    moqt_payload::MOQTPayload,
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

pub(super) struct PublishOk {
    pub(super) request_id: u64,
    pub(super) forward: bool,
    pub(super) subscriber_priority: u8,
    pub(super) group_order: GroupOrder,
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
        let forward_u8 = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("forward")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let forward = util::u8_to_bool(forward_u8)?;
        let subscriber_priority = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("subscriber priority")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let group_order_u8 = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("group order")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let group_order = GroupOrder::try_from(group_order_u8)
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let filter_type = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("filter type")
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
        payload.extend(write_variable_integer(self.parameters.len() as u64));
        // Parameters
        for param in &self.parameters {
            param.packetize(&mut payload);
        }

        tracing::trace!("Packetized Publish message.");
        add_payload_length(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {

        use crate::modules::moqt::messages::{
            control_messages::{
                enums::FilterType,
                group_order::GroupOrder,
                location::Location,
                publish_ok::PublishOk,
                version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
            },
            moqt_message::MOQTMessage,
        };

        #[test]
        fn packetize_and_depacketize_absolute_start() {
            let publish_ok_message = PublishOk {
                request_id: 1,
                forward: true,
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending, // Ascending
                filter_type: FilterType::AbsoluteStart,
                start_location: Some(Location {
                    group_id: 10,
                    object_id: 5,
                }),
                end_group: None,
                parameters: vec![VersionSpecificParameter::AuthorizationInfo(
                    AuthorizationInfo::new("token".to_string()),
                )],
            };

            let mut buf = publish_ok_message.packetize();
            let depacketized_message = PublishOk::depacketize(&mut buf).unwrap();

            assert_eq!(
                publish_ok_message.request_id,
                depacketized_message.request_id
            );
            assert_eq!(publish_ok_message.forward, depacketized_message.forward);
            assert_eq!(
                publish_ok_message.subscriber_priority,
                depacketized_message.subscriber_priority
            );
            assert_eq!(
                publish_ok_message.group_order,
                depacketized_message.group_order
            );
            assert_eq!(
                publish_ok_message.filter_type,
                depacketized_message.filter_type
            );
            let start_location = publish_ok_message.start_location.unwrap();
            let depacketized_start_location = depacketized_message.start_location.unwrap();
            assert_eq!(
                start_location.group_id,
                depacketized_start_location.group_id
            );
            assert_eq!(
                start_location.object_id,
                depacketized_start_location.object_id
            );
            assert!(depacketized_message.end_group.is_none());
            assert_eq!(
                publish_ok_message.parameters,
                depacketized_message.parameters
            );
        }

        #[test]
        fn packetize_and_depacketize_absolute_range() {
            let publish_ok_message = PublishOk {
                request_id: 2,
                forward: false,
                subscriber_priority: 64,
                group_order: GroupOrder::Descending, // Descending
                filter_type: FilterType::AbsoluteRange,
                start_location: Some(Location {
                    group_id: 20,
                    object_id: 15,
                }),
                end_group: Some(30),
                parameters: vec![],
            };

            let mut buf = publish_ok_message.packetize();
            let depacketized_message = PublishOk::depacketize(&mut buf).unwrap();

            assert_eq!(
                publish_ok_message.request_id,
                depacketized_message.request_id
            );
            assert_eq!(publish_ok_message.forward, depacketized_message.forward);
            assert_eq!(
                publish_ok_message.subscriber_priority,
                depacketized_message.subscriber_priority
            );
            assert_eq!(
                publish_ok_message.group_order,
                depacketized_message.group_order
            );
            assert_eq!(
                publish_ok_message.filter_type,
                depacketized_message.filter_type
            );
            let start_location = publish_ok_message.start_location.unwrap();
            let depacketized_start_location = depacketized_message.start_location.unwrap();
            assert_eq!(
                start_location.group_id,
                depacketized_start_location.group_id
            );
            assert_eq!(
                start_location.object_id,
                depacketized_start_location.object_id
            );
            assert_eq!(publish_ok_message.end_group, depacketized_message.end_group);
            assert!(depacketized_message.parameters.is_empty());
        }

        #[test]
        fn packetize_and_depacketize_latest_group() {
            let publish_ok_message = PublishOk {
                request_id: 3,
                forward: true,
                subscriber_priority: 0,
                group_order: GroupOrder::Original, // Original
                filter_type: FilterType::LatestGroup,
                start_location: None,
                end_group: None,
                parameters: vec![],
            };

            let mut buf = publish_ok_message.packetize();
            let depacketized_message = PublishOk::depacketize(&mut buf).unwrap();

            assert_eq!(
                publish_ok_message.request_id,
                depacketized_message.request_id
            );
            assert_eq!(
                publish_ok_message.filter_type,
                depacketized_message.filter_type
            );
            let depacketized_start_location = depacketized_message.start_location;
            assert!(depacketized_start_location.is_none());
            assert!(depacketized_message.end_group.is_none());
            assert!(depacketized_message.parameters.is_empty());
        }
    }
}
