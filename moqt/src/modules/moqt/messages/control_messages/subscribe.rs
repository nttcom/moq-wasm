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
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
use anyhow::Context;
use bytes::BytesMut;
use tracing;

pub enum FilterTypePair {
    LatestObject,
    LatestGroup,
    AbsoluteStart(Location),
    AbsoluteRange(Location, u64),
}

#[derive(Debug, PartialEq)]
pub struct Subscribe {
    pub(crate) request_id: u64,
    pub(crate) track_namespace: Vec<String>,
    pub(crate) track_name: String,
    pub(crate) subscriber_priority: u8,
    pub(crate) group_order: GroupOrder,
    pub(crate) forward: bool,
    pub(crate) filter_type: FilterType,
    pub(crate) start_location: Option<Location>,
    pub(crate) end_group: Option<u64>,
    pub(crate) subscribe_parameters: Vec<VersionSpecificParameter>,
}

impl MOQTMessage for Subscribe {
    fn depacketize(buf: &mut BytesMut) -> Result<Self, MOQTMessageError> {
        if !validate_payload_length(buf) {
            return Err(MOQTMessageError::ProtocolViolation);
        }
        let subscribe_id = read_variable_integer_from_buffer(buf)
            .context("subscribe id")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
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
        tracing::warn!("qqq track name: {:?}", track_name);
        tracing::warn!("qqq buffer: {:?}", buf);
        let subscriber_priority = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .context("subscriber priority")
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let group_order_u8 = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .context("group order")
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;

        // Values larger than 0x2 are a Protocol Violation.
        let group_order = match GroupOrder::try_from(group_order_u8).context("group order") {
            Ok(group_order) => group_order,
            Err(_) => {
                // TODO: return Termination Error Code
                return Err(MOQTMessageError::ProtocolViolation);
            }
        };
        let forward_u8 = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("forward")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let forward = util::u8_to_bool(forward_u8)?;
        let filter_type_u64 = read_variable_integer_from_buffer(buf)
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let filter_type = FilterType::try_from(filter_type_u64 as u8)
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        // A filter type other than the above MUST be treated as error.
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

        tracing::trace!("Depacketized Subscribe message.");

        Ok(Subscribe {
            request_id: subscribe_id,
            track_namespace: track_namespace_tuple,
            track_name,
            subscriber_priority,
            group_order,
            forward,
            filter_type,
            start_location,
            end_group,
            subscribe_parameters,
        })
    }

    fn packetize(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.extend(write_variable_integer(self.request_id));
        // Track Namespace Number of elements
        let track_namespace_tuple_length = self.track_namespace.len();
        payload.extend(write_variable_integer(track_namespace_tuple_length as u64));
        for track_namespace in &self.track_namespace {
            // Track Namespace
            payload.extend(write_variable_bytes(&track_namespace.as_bytes().to_vec()));
        }
        payload.extend(write_variable_bytes(&self.track_name.as_bytes().to_vec()));
        payload.extend(write_variable_integer(self.subscriber_priority as u64));
        payload.extend(u8::from(self.group_order).to_be_bytes());
        payload.extend((self.forward as u8).to_be_bytes());
        payload.extend(write_variable_integer(u8::from(self.filter_type) as u64));
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
        payload.extend(write_variable_integer(
            self.subscribe_parameters.len() as u64
        ));
        for version_specific_parameter in &self.subscribe_parameters {
            version_specific_parameter.packetize(&mut payload);
        }

        tracing::trace!("Packetized Subscribe OK message.");

        add_payload_length(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use bytes::BytesMut;

        use crate::modules::moqt::messages::{
            control_messages::{
                location::Location,
                subscribe::{FilterType, GroupOrder, Subscribe},
                version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
            },
            moqt_message::MOQTMessage,
        };

        #[test]
        fn packetize_latest_group() {
            let request_id = 0;
            let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
            let track_name = "track_name".to_string();
            let subscriber_priority = 0;
            let group_order = GroupOrder::Ascending;
            let forward = false;
            let filter_type = FilterType::LatestGroup;
            let start_location = None;
            let end_group = None;
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let subscribe_parameters = vec![version_specific_parameter];

            let subscribe = Subscribe {
                request_id,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                forward,
                filter_type,
                start_location,
                end_group,
                subscribe_parameters,
            };

            let buf = subscribe.packetize();

            let expected_bytes_array = [
                35, // Message Length(i)
                0,  // Subscribe ID (i)
                0,  // Track Alias (i)
                2,  // Track Namespace(tuple): Number of elements
                4,  // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                4,   // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                10,  // Track Name (b): Length
                116, 114, 97, 99, 107, 95, 110, 97, 109,
                101, // Track Name (b): Value("track_name")
                0,   // Subscriber Priority (8)
                1,   // Group Order (8): Assending
                0,   // Forward(8)
                1,   // Filter Type (i): LatestGroup
                1,   // Track Request Parameters (..): Number of Parameters
                2,   // Parameter Type (i): AuthorizationInfo
                4,   // Parameter Length (i)
                116, 101, 115, 116, // Parameter Value (..): test
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn packetize_absolute_start() {
            let request_id = 0;
            let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
            let track_name = "track_name".to_string();
            let subscriber_priority = 0;
            let group_order = GroupOrder::Descending;
            let forward = false;
            let filter_type = FilterType::AbsoluteStart;
            let start_location = Some(Location {
                group_id: 10,
                object_id: 20,
            });
            let end_group = None;
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let subscribe_parameters = vec![version_specific_parameter];

            let subscribe = Subscribe {
                request_id,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                forward,
                filter_type,
                start_location,
                end_group,
                subscribe_parameters,
            };

            let buf = subscribe.packetize();

            let expected_bytes_array = [
                37, // Message Length(i)
                0,  // Subscribe ID (i)
                2,  // Track Namespace(tuple): Number of elements
                4,  // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                4,   // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                10,  // Track Name (b): Length
                116, 114, 97, 99, 107, 95, 110, 97, 109,
                101, // Track Name (b): Value("track_name")
                0,   // Subscriber Priority (8)
                2,   // Group Order (8): Descending
                0,   // Forward(8)
                3,   // Filter Type (i): AbsoluteStart
                10,  // Location: group id (i)
                20,  // Location: object id (i)
                1,   // Track Request Parameters (..): Number of Parameters
                2,   // Parameter Type (i): AuthorizationInfo
                4,   // Parameter Length
                116, 101, 115, 116, // Parameter Value (..): test
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn packetize_absolute_range() {
            let request_id = 0;
            let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
            let track_name = "track_name".to_string();
            let subscriber_priority = 0;
            let group_order = GroupOrder::Ascending;
            let forward = true;
            let filter_type = FilterType::AbsoluteRange;
            let start_location = Some(Location {
                group_id: 10,
                object_id: 20,
            });
            let end_group = Some(10);
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let subscribe_parameters = vec![version_specific_parameter];

            let subscribe = Subscribe {
                request_id,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                forward,
                filter_type,
                start_location,
                end_group,
                subscribe_parameters,
            };

            let buf = subscribe.packetize();

            let expected_bytes_array = [
                38, // Message Length(i)
                0,  // Subscribe ID (i)
                2,  // Track Namespace(tuple): Number of elements
                4,  // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                4,   // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                10,  // Track Name (b): Length
                116, 114, 97, 99, 107, 95, 110, 97, 109,
                101, // Track Name (b): Value("track_name")
                0,   // Subscriber Priority (8)
                1,   // Group Order (8): Assending
                1,   // Forward(8)
                4,   // Filter Type (i): AbsoluteRange
                10,  // Location: group id (i)
                20,  // Location: object id (i)
                10,  // End Group (i)
                1,   // Track Request Parameters (..): Number of Parameters
                2,   // Parameter Type (i): AuthorizationInfo
                4,   // Parameter Length
                116, 101, 115, 116, // Parameter Value (..): test
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize_latest_group() {
            let bytes_array = [
                35, // Message Length(i)
                0,  // Subscribe ID (i)
                2,  // Track Namespace(tuple): Number of elements
                4,  // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                4,   // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                10,  // Track Name (b): Length
                116, 114, 97, 99, 107, 95, 110, 97, 109,
                101, // Track Name (b): Value("track_name")
                0,   // Subscriber Priority (8)
                1,   // Group Order (8): Assending
                1,   // Forward(8)
                1,   // Filter Type (i): LatestGroup
                1,   // Track Request Parameters (..): Number of Parameters
                2,   // Parameter Type (i): AuthorizationInfo
                4,   // Parameter Length (i)
                116, 101, 115, 116, // Parameter Value (..): test
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let depacketized_subscribe = Subscribe::depacketize(&mut buf).unwrap();

            let request_id = 0;
            let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
            let track_name = "track_name".to_string();
            let subscriber_priority = 0;
            let group_order = GroupOrder::Ascending;
            let forward = true;
            let filter_type = FilterType::LatestGroup;
            let start_location = None;
            let end_group = None;
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let subscribe_parameters = vec![version_specific_parameter];
            let expected_subscribe = Subscribe {
                request_id,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                forward,
                filter_type,
                start_location,
                end_group,
                subscribe_parameters,
            };

            assert_eq!(depacketized_subscribe, expected_subscribe);
        }

        #[test]
        fn depacketize_absolute_start() {
            let bytes_array = [
                37, // Message Length(i)
                0,  // Subscribe ID (i)
                2,  // Track Namespace(tuple): Number of elements
                4,  // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                4,   // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                10,  // Track Name (b): Length
                116, 114, 97, 99, 107, 95, 110, 97, 109,
                101, // Track Name (b): Value("track_name")
                0,   // Subscriber Priority (8)
                1,   // Group Order (8): Assending
                0,   // Forward(8)
                3,   // Filter Type (i): AbsoluteStart
                5,   // Location: group id (i)
                10,  // Location: object id (i)
                1,   // Track Request Parameters (..): Number of Parameters
                2,   // Parameter Type (i): AuthorizationInfo
                4,   // Parameter Length
                116, 101, 115, 116, // Parameter Value (..): test
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let depacketized_subscribe = Subscribe::depacketize(&mut buf).unwrap();

            let request_id = 0;
            let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
            let track_name = "track_name".to_string();
            let subscriber_priority = 0;
            let group_order = GroupOrder::Ascending;
            let forward = false;
            let filter_type = FilterType::AbsoluteStart;
            let start_location = Some(Location {
                group_id: 5,
                object_id: 10,
            });
            let end_group = None;
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let subscribe_parameters = vec![version_specific_parameter];
            let expected_subscribe = Subscribe {
                request_id,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                forward,
                filter_type,
                start_location,
                end_group,
                subscribe_parameters,
            };

            assert_eq!(depacketized_subscribe, expected_subscribe);
        }

        #[test]
        fn depacketize_absolute_range() {
            let bytes_array = [
                38, // Message Length(i)
                0,  // Subscribe ID (i)
                2,  // Track Namespace(tuple): Number of elements
                4,  // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                4,   // Track Namespace(b): Length
                116, 101, 115, 116, // Track Namespace(b): Value("test")
                10,  // Track Name (b): Length
                116, 114, 97, 99, 107, 95, 110, 97, 109,
                101, // Track Name (b): Value("track_name")
                0,   // Subscriber Priority (8)
                1,   // Group Order (8): Assending
                0,   // Forward(8)
                4,   // Filter Type (i): AbsoluteRange
                5,   // Location: group id (i)
                10,  // Location: object id (i)
                10,  // End Group (i)
                1,   // Track Request Parameters (..): Number of Parameters
                2,   // Parameter Type (i): AuthorizationInfo
                4,   // Parameter Length
                116, 101, 115, 116, // Parameter Value (..): test
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let depacketized_subscribe = Subscribe::depacketize(&mut buf);
            let depacketized_subscribe = match depacketized_subscribe {
                Ok(s) => s,
                Err(e) => {
                    panic!("Failed to depacketize: {:?}", e)
                }
            };

            let request_id = 0;
            let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
            let track_name = "track_name".to_string();
            let subscriber_priority = 0;
            let group_order = GroupOrder::Ascending;
            let forward = false;
            let filter_type = FilterType::AbsoluteRange;
            let start_location = Some(Location {
                group_id: 5,
                object_id: 10,
            });
            let end_group = Some(10);
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let subscribe_parameters = vec![version_specific_parameter];
            let expected_subscribe = Subscribe {
                request_id,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                forward,
                filter_type,
                start_location,
                end_group,
                subscribe_parameters,
            };
            assert_eq!(depacketized_subscribe, expected_subscribe);
        }
    }
}
