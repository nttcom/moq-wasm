use anyhow::Context;
use bytes::BytesMut;

use crate::modules::moqt::messages::{
    control_messages::{
        group_order::GroupOrder,
        location::Location,
        util::{self, add_payload_length, validate_payload_length},
        version_specific_parameters::VersionSpecificParameter,
    },
    moqt_message::MOQTMessage,
    moqt_message_error::MOQTMessageError,
    moqt_payload::MOQTPayload,
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

pub enum ContentExistsPair {
    False,
    True(Location),
}

pub(crate) struct Publish {
    pub(crate) request_id: u64,
    pub(crate) track_namespace_tuple: Vec<String>,
    pub(crate) track_name: String,
    pub(crate) track_alias: u64,
    pub(crate) group_order: GroupOrder,
    pub(crate) content_exists: bool,
    pub(crate) largest_location: Option<Location>,
    pub(crate) forward: bool,
    pub(crate) parameters: Vec<VersionSpecificParameter>,
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
        .context("track name")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let track_alias = match read_variable_integer_from_buffer(buf) {
            Ok(v) => v,
            Err(_) => return Err(MOQTMessageError::ProtocolViolation),
        };
        let group_order_u8 = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("track namespace length")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let group_order = GroupOrder::try_from(group_order_u8)
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let content_exists_u8 = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("track namespace length")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let content_exists = util::u8_to_bool(content_exists_u8)?;
        // location
        let largest_location = if content_exists {
            Some(Location::depacketize(buf)?)
        } else {
            None
        };

        let forward_u8 = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("track namespace length")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let forward = util::u8_to_bool(forward_u8)?;
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
        payload.extend(write_variable_bytes(&self.track_name.as_bytes().to_vec()));

        payload.extend(write_variable_integer(self.track_alias));
        payload.extend(write_variable_integer(self.group_order as u64));

        payload.extend(write_variable_integer(self.content_exists as u64));
        // location
        if let Some(location) = &self.largest_location {
            let bytes = location.packetize();
            payload.extend(bytes);
        }
        payload.extend(write_variable_integer(self.forward as u64));

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
                group_order::GroupOrder,
                location::Location,
                publish::Publish,
                version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
            },
            moqt_message::MOQTMessage,
        };

        #[test]
        fn packetize_and_depacketize_with_location_and_params() {
            let publish_message = Publish {
                request_id: 1,
                track_namespace_tuple: vec!["moq".to_string(), "news".to_string()],
                track_name: "video".to_string(),
                track_alias: 2,
                group_order: GroupOrder::Ascending, // Ascending
                content_exists: true,
                largest_location: Some(Location {
                    group_id: 10,
                    object_id: 5,
                }),
                forward: true,
                parameters: vec![VersionSpecificParameter::AuthorizationInfo(
                    AuthorizationInfo::new("token".to_string()),
                )],
            };

            let mut buf = publish_message.packetize();

            // depacketize
            let depacketized_message = Publish::depacketize(&mut buf).unwrap();

            assert_eq!(publish_message.request_id, depacketized_message.request_id);
            assert_eq!(
                publish_message.track_namespace_tuple,
                depacketized_message.track_namespace_tuple
            );
            assert_eq!(publish_message.track_name, depacketized_message.track_name);
            assert_eq!(
                publish_message.track_alias,
                depacketized_message.track_alias
            );
            assert_eq!(
                publish_message.group_order,
                depacketized_message.group_order
            );
            assert_eq!(
                publish_message.content_exists,
                depacketized_message.content_exists
            );
            let largest_location = publish_message.largest_location.unwrap();
            let depacketized_largest_location = depacketized_message.largest_location.unwrap();
            assert_eq!(
                largest_location.group_id,
                depacketized_largest_location.group_id
            );
            assert_eq!(
                largest_location.object_id,
                depacketized_largest_location.object_id
            );
            assert_eq!(publish_message.forward, depacketized_message.forward);
            assert_eq!(publish_message.parameters, depacketized_message.parameters);
        }

        #[test]
        fn packetize_and_depacketize_without_location_or_params() {
            let publish_message = Publish {
                request_id: 1,
                track_namespace_tuple: vec!["moq".to_string()],
                track_name: "audio".to_string(),
                track_alias: 3,
                group_order: GroupOrder::Descending, // Descending
                content_exists: false,
                largest_location: None,
                forward: false,
                parameters: vec![],
            };

            let mut buf = publish_message.packetize();

            // depacketize
            let depacketized_message = Publish::depacketize(&mut buf).unwrap();

            assert_eq!(publish_message.request_id, depacketized_message.request_id);
            assert_eq!(
                publish_message.track_namespace_tuple,
                depacketized_message.track_namespace_tuple
            );
            assert_eq!(publish_message.track_name, depacketized_message.track_name);
            assert_eq!(
                publish_message.track_alias,
                depacketized_message.track_alias
            );
            assert_eq!(
                publish_message.group_order,
                depacketized_message.group_order
            );
            assert_eq!(
                publish_message.content_exists,
                depacketized_message.content_exists
            );
            assert!(depacketized_message.largest_location.is_none());
            assert_eq!(publish_message.forward, depacketized_message.forward);
            assert!(depacketized_message.parameters.is_empty());
        }

        #[test]
        fn packetize_check_bytes() {
            let publish_message = Publish {
                request_id: 1,
                track_namespace_tuple: vec!["moq".to_string()],
                track_name: "video".to_string(),
                track_alias: 2,
                group_order: GroupOrder::Ascending,
                content_exists: false,
                largest_location: None,
                forward: true,
                parameters: vec![],
            };

            let buf = publish_message.packetize();

            let expected_bytes = vec![
                17, 1, 1, 3, b'm', b'o', b'q', 5, b'v', b'i', b'd', b'e', b'o', 2, 1, 0, 1, 0,
            ];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }
    }
}
