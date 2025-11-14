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
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
use anyhow::Context;
use bytes::BytesMut;

pub enum ContentExistsPair {
    False,
    True(Location),
}

#[derive(Debug, PartialEq, Clone)]
pub struct SubscribeOk {
    pub(crate) request_id: u64,
    pub(crate) track_alias: u64,
    pub(crate) expires: u64,
    pub(crate) group_order: GroupOrder,
    pub(crate) content_exists: bool,
    pub(crate) largest_location: Option<Location>,
    pub(crate) subscribe_parameters: Vec<VersionSpecificParameter>,
}

impl MOQTMessage for SubscribeOk {
    fn depacketize(buf: &mut BytesMut) -> Result<Self, MOQTMessageError> {
        if !validate_payload_length(buf) {
            return Err(MOQTMessageError::ProtocolViolation);
        }
        let request_id = read_variable_integer_from_buffer(buf)
            .context("subscribe_id")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let track_alias = read_variable_integer_from_buffer(buf)
            .context("track_alias")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let expires = read_variable_integer_from_buffer(buf)
            .context("expires")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let group_order_u64 = read_variable_integer_from_buffer(buf)
            .context("group order")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let group_order_u8 = u8::try_from(group_order_u64)
            .context("group order")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;

        // Values larger than 0x2 are a Protocol Violation.
        let group_order = match GroupOrder::try_from(group_order_u8).context("group order") {
            Ok(group_order) => group_order,
            Err(_) => {
                return Err(MOQTMessageError::ProtocolViolation);
            }
        };

        let content_exists_u8 = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .context("content_exists_u8")
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;

        let content_exists = util::u8_to_bool(content_exists_u8)?;

        let largest_location = if content_exists {
            Some(Location::depacketize(buf)?)
        } else {
            None
        };

        let number_of_parameters = read_variable_integer_from_buffer(buf)
            .context("number of parameters")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let mut subscribe_parameters = Vec::new();
        for _ in 0..number_of_parameters {
            let version_specific_parameter = VersionSpecificParameter::depacketize(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?;
            if let VersionSpecificParameter::Unknown(code) = version_specific_parameter {
                tracing::warn!("unknown track request parameter {}", code);
            } else {
                subscribe_parameters.push(version_specific_parameter);
            }
        }

        Ok(SubscribeOk {
            request_id,
            track_alias,
            expires,
            group_order,
            content_exists,
            largest_location,
            subscribe_parameters,
        })
    }

    fn packetize(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.extend(write_variable_integer(self.request_id));
        payload.extend(write_variable_integer(self.track_alias));
        payload.extend(write_variable_integer(self.expires));
        payload.extend(u8::from(self.group_order).to_be_bytes());
        payload.extend(u8::from(self.content_exists).to_be_bytes());
        if self.content_exists {
            let largest_location = self.largest_location.as_ref().unwrap();
            payload.extend(write_variable_integer(largest_location.group_id));
            payload.extend(write_variable_integer(largest_location.object_id));
        }
        payload.extend(write_variable_integer(
            self.subscribe_parameters.len() as u64
        ));
        for version_specific_parameter in &self.subscribe_parameters {
            version_specific_parameter.packetize(&mut payload);
        }
        add_payload_length(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::messages::{
            control_messages::{
                location::Location,
                subscribe_ok::{GroupOrder, SubscribeOk},
                version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
            },
            moqt_message::MOQTMessage,
        };
        use bytes::BytesMut;

        #[test]
        fn packetize_content_not_exists() {
            let request_id = 0;
            let track_alias = 1;
            let expires = 1;
            let group_order = GroupOrder::Ascending;
            let content_exists = false;
            let largest_location = None;
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let subscribe_parameters = vec![version_specific_parameter];

            let subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                largest_location,
                subscribe_parameters,
            };
            let buf = subscribe_ok.packetize();

            let expected_bytes_array = [
                12, // Message Length(i)
                0,  // Request ID (i)
                1,  // Track alias (i)
                1,  // Expires (i)
                1,  // Group Order (8)
                0,  // Content Exists (f)
                1,  // Track Request Parameters (..): Number of Parameters
                2,  // Parameter Type (i): AuthorizationInfo
                4,  // Parameter Length (i)
                116, 101, 115, 116, // Parameter Value (..): test
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn packetize_content_exists() {
            let request_id = 0;
            let track_alias = 2;
            let expires = 1;
            let group_order = GroupOrder::Descending;
            let content_exists = true;
            let largest_location = Some(Location {
                group_id: 10,
                object_id: 20,
            });
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let subscribe_parameters = vec![version_specific_parameter];

            let subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                largest_location,
                subscribe_parameters,
            };
            let buf = subscribe_ok.packetize();

            let expected_bytes_array = [
                14, // Message Length(i)
                0,  // Request ID (i)
                2,  // Track alias (i)
                1,  // Expires (i)
                2,  // Group Order (8)
                1,  // Content Exists (f)
                10, // Largest Group ID (i)
                20, // Largest Object ID (i)
                1,  // Track Request Parameters (..): Number of Parameters
                2,  // Parameter Type (i): AuthorizationInfo
                4,  // Parameter Length (i)
                116, 101, 115, 116, // Parameter Value (..): test
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize_content_not_exists() {
            let bytes_array = [
                12, // Message Length(i)
                0,  // Request ID (i)
                1,  // Track alias (i)
                1,  // Expires (i)
                2,  // Group Order (8)
                0,  // Content Exists (f)
                1,  // Track Request Parameters (..): Number of Parameters
                2,  // Parameter Type (i): AuthorizationInfo
                4,  // Parameter Length (i)
                116, 101, 115, 116, // Parameter Value (..): test
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let depacketized_subscribe_ok = SubscribeOk::depacketize(&mut buf).unwrap();

            let request_id = 0;
            let track_alias = 1;
            let expires = 1;
            let group_order = GroupOrder::Descending;
            let content_exists = false;
            let largest_location = None;
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let subscribe_parameters = vec![version_specific_parameter];

            let expected_subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                largest_location,
                subscribe_parameters,
            };

            assert_eq!(depacketized_subscribe_ok, expected_subscribe_ok);
        }

        #[test]
        fn depacketize_content_exists() {
            let bytes_array = [
                14, // Message Length(i)
                0,  // Request ID (i)
                2,  // Track alias (i)
                1,  // Expires (i)
                1,  // Group Order (8)
                1,  // Content Exists (f)
                0,  // Largest Group ID (i)
                5,  // Largest Object ID (i)
                1,  // Track Request Parameters (..): Number of Parameters
                2,  // Parameter Type (i): AuthorizationInfo
                4,  // Parameter Length (i)
                116, 101, 115, 116, // Parameter Value (..): test
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let depacketized_subscribe_ok = SubscribeOk::depacketize(&mut buf).unwrap();

            let request_id = 0;
            let track_alias = 2;
            let expires = 1;
            let group_order = GroupOrder::Ascending;
            let content_exists = true;
            let largest_location = Some(Location {
                group_id: 0,
                object_id: 5,
            });
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let subscribe_parameters = vec![version_specific_parameter];

            let expected_subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                largest_location,
                subscribe_parameters,
            };

            assert_eq!(depacketized_subscribe_ok, expected_subscribe_ok);
        }
    }
}
