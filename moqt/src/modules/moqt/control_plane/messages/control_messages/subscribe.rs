use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::messages::{
        control_messages::{
            enums::FilterType,
            group_order::GroupOrder,
            util::{self, add_payload_length, validate_payload_length},
            version_specific_parameters::VersionSpecificParameter,
        },
        moqt_payload::MOQTPayload,
    },
};
use bytes::{Buf, BufMut, BytesMut};
use tracing;

#[derive(Debug, PartialEq)]
pub struct Subscribe {
    pub(crate) request_id: u64,
    pub(crate) track_namespace: Vec<String>,
    pub(crate) track_name: String,
    pub(crate) subscriber_priority: u8,
    pub(crate) group_order: GroupOrder,
    pub(crate) forward: bool,
    pub(crate) filter_type: FilterType,
    pub(crate) subscribe_parameters: Vec<VersionSpecificParameter>,
}

impl Subscribe {
    pub(crate) fn decode(buf: &mut BytesMut) -> Option<Self> {
        if !validate_payload_length(buf) {
            return None;
        }
        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        let track_namespace_tuple_length = buf
            .try_get_varint()
            .log_context("track namespace length")
            .ok()?;
        let mut track_namespace_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let track_namespace = buf.try_get_string().log_context("track namespace").ok()?;
            track_namespace_tuple.push(track_namespace);
        }
        let track_name = buf.try_get_string().log_context("track name").ok()?;
        let subscriber_priority = buf.try_get_u8().log_context("subscriber priority").ok()?;
        let group_order_u8 = buf.try_get_u8().log_context("group order u8").ok()?;
        let group_order = GroupOrder::try_from(group_order_u8)
            .log_context("group order")
            .ok()?;
        let forward_u8 = buf.try_get_u8().log_context("forward u8").ok()?;
        let forward = util::u8_to_bool(forward_u8).log_context("forward").ok()?;
        let filter_type = FilterType::decode(buf)?;
        let number_of_parameters = buf
            .try_get_varint()
            .log_context("number of parameters")
            .ok()?;
        let mut subscribe_parameters = Vec::new();
        for _ in 0..number_of_parameters {
            let version_specific_parameter = VersionSpecificParameter::depacketize(buf).ok()?;
            if let VersionSpecificParameter::Unknown(code) = version_specific_parameter {
                tracing::warn!("unknown track request parameter {}", code);
            } else {
                subscribe_parameters.push(version_specific_parameter);
            }
        }
        tracing::trace!("Depacketized Subscribe message.");

        Some(Subscribe {
            request_id,
            track_namespace: track_namespace_tuple,
            track_name,
            subscriber_priority,
            group_order,
            forward,
            filter_type,
            subscribe_parameters,
        })
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        // Track Namespace Number of elements
        let track_namespace_tuple_length = self.track_namespace.len();
        payload.put_varint(track_namespace_tuple_length as u64);
        self.track_namespace.iter().for_each(|track_namespace| {
            payload.put_string(track_namespace);
        });
        payload.put_string(&self.track_name);
        payload.put_u8(self.subscriber_priority);
        payload.put_u8(self.group_order as u8);
        payload.put_u8(self.forward as u8);
        payload.unsplit(self.filter_type.encode());
        payload.put_varint(self.subscribe_parameters.len() as u64);
        for version_specific_parameter in &self.subscribe_parameters {
            version_specific_parameter.packetize(&mut payload);
        }

        tracing::trace!("Packetized Subscribe message.");

        add_payload_length(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use bytes::BytesMut;

        use crate::modules::moqt::control_plane::messages::control_messages::{
            location::Location,
            subscribe::{FilterType, GroupOrder, Subscribe},
            version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
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
                subscribe_parameters,
            };

            let buf = subscribe.encode();

            let expected_bytes_array = [
                0,  // Message Length(16)
                34, // Message Length(16)
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
            let filter_type = FilterType::AbsoluteStart {
                location: Location {
                    group_id: 10,
                    object_id: 20,
                },
            };
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
                subscribe_parameters,
            };

            let buf = subscribe.encode();

            let expected_bytes_array = [
                0,  // Message Length(16)
                36, // Message Length(16)
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
            let filter_type = FilterType::AbsoluteRange {
                location: Location {
                    group_id: 10,
                    object_id: 20,
                },
                end_group: 10,
            };
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
                subscribe_parameters,
            };

            let buf = subscribe.encode();

            let expected_bytes_array = [
                0,  // Message Length(16)
                37, // Message Length(16)
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
                0,  // Message Length(16)
                34, // Message Length(16)
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
            let depacketized_subscribe = Subscribe::decode(&mut buf).unwrap();

            let request_id = 0;
            let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
            let track_name = "track_name".to_string();
            let subscriber_priority = 0;
            let group_order = GroupOrder::Ascending;
            let forward = true;
            let filter_type = FilterType::LatestGroup;
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
                subscribe_parameters,
            };

            assert_eq!(depacketized_subscribe, expected_subscribe);
        }

        #[test]
        fn depacketize_absolute_start() {
            let bytes_array = [
                0,  // Message Length(16)
                36, // Message Length(16)
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
            let depacketized_subscribe = Subscribe::decode(&mut buf).unwrap();

            let request_id = 0;
            let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
            let track_name = "track_name".to_string();
            let subscriber_priority = 0;
            let group_order = GroupOrder::Ascending;
            let forward = false;
            let filter_type = FilterType::AbsoluteStart {
                location: Location {
                    group_id: 5,
                    object_id: 10,
                },
            };
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
                subscribe_parameters,
            };

            assert_eq!(depacketized_subscribe, expected_subscribe);
        }

        #[test]
        fn depacketize_absolute_range() {
            let bytes_array = [
                0,  // Message Length(16)
                37, // Message Length(16)
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
            let depacketized_subscribe = Subscribe::decode(&mut buf);
            let depacketized_subscribe = match depacketized_subscribe {
                Some(s) => s,
                None => {
                    panic!("Failed to depacketize")
                }
            };

            let request_id = 0;
            let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
            let track_name = "track_name".to_string();
            let subscriber_priority = 0;
            let group_order = GroupOrder::Ascending;
            let forward = false;
            let filter_type = FilterType::AbsoluteRange {
                location: Location {
                    group_id: 5,
                    object_id: 10,
                },
                end_group: 10,
            };
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
                subscribe_parameters,
            };
            assert_eq!(depacketized_subscribe, expected_subscribe);
        }
    }
}
