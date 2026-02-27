use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::{
        key_value_pair::{KeyValuePair, VariantType},
        messages::parameters::{
            authorization_token::AuthorizationToken, filter_type::FilterType,
            group_order::GroupOrder,
        },
        util,
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
    pub(crate) authorization_tokens: Vec<AuthorizationToken>,
    pub(crate) delivery_timeout: Option<u64>,
}

impl Subscribe {
    pub(crate) fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
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
        let mut parameters = vec![];
        for _ in 0..number_of_parameters {
            let params = KeyValuePair::decode(buf)?;
            parameters.push(params);
        }
        let authorization_tokens = parameters
            .iter()
            .filter(|kv_pair| kv_pair.key == 0x03)
            .filter_map(|kv_pair| match &kv_pair.value {
                VariantType::Odd(value) => {
                    let mut value = std::io::Cursor::new(&value[..]);
                    AuthorizationToken::decode(&mut value)
                }
                VariantType::Even(_) => unreachable!(),
            })
            .collect();
        let delivery_timeout =
            parameters
                .iter()
                .find(|kv_pair| kv_pair.key == 0x02)
                .map(|kv_pair| match kv_pair.value {
                    VariantType::Odd(_) => unreachable!(),
                    VariantType::Even(value) => value,
                });
        tracing::trace!("Depacketized Subscribe message.");

        Some(Subscribe {
            request_id,
            track_namespace: track_namespace_tuple,
            track_name,
            subscriber_priority,
            group_order,
            forward,
            filter_type,
            authorization_tokens,
            delivery_timeout,
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
        let mut number_of_parameters = 0;
        let mut parameters_payload = BytesMut::new();
        for token in &self.authorization_tokens {
            let token_payload = token.encode();
            parameters_payload.unsplit(token_payload);
            number_of_parameters += 1;
        }
        if let Some(delivery_timeout) = self.delivery_timeout {
            let delivery_timeout_payload = KeyValuePair {
                key: 0x02,
                value: VariantType::Even(delivery_timeout),
            }
            .encode();
            parameters_payload.unsplit(delivery_timeout_payload);
            number_of_parameters += 1;
        }
        payload.put_varint(number_of_parameters);
        payload.unsplit(parameters_payload);

        tracing::trace!("Packetized Subscribe message.");
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use bytes::BytesMut;

        use crate::modules::moqt::control_plane::control_messages::messages::{
            parameters::{filter_type::FilterType, group_order::GroupOrder, location::Location},
            subscribe::Subscribe,
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

            let subscribe = Subscribe {
                request_id,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                forward,
                filter_type,
                authorization_tokens: vec![],
                delivery_timeout: None,
            };

            let buf = subscribe.encode();

            let expected_bytes_array = [
                0, // Subscribe ID (i)
                2, // Track Namespace(tuple): Number of elements
                4, // Track Namespace(b): Length
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
                0,   // Track Request Parameters (..): Number of Parameters
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

            let subscribe = Subscribe {
                request_id,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                forward,
                filter_type,
                authorization_tokens: vec![],
                delivery_timeout: None,
            };

            let buf = subscribe.encode();

            let expected_bytes_array = [
                0, // Subscribe ID (i)
                2, // Track Namespace(tuple): Number of elements
                4, // Track Namespace(b): Length
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
                0,   // Track Request Parameters (..): Number of Parameters
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

            let subscribe = Subscribe {
                request_id,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                forward,
                filter_type,
                authorization_tokens: vec![],
                delivery_timeout: None,
            };

            let buf = subscribe.encode();

            let expected_bytes_array = [
                0, // Subscribe ID (i)
                2, // Track Namespace(tuple): Number of elements
                4, // Track Namespace(b): Length
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
                0,   // Track Request Parameters (..): Number of Parameters
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize_latest_group() {
            let bytes_array = [
                0, // Subscribe ID (i)
                2, // Track Namespace(tuple): Number of elements
                4, // Track Namespace(b): Length
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
                0,   // Track Request Parameters (..): Number of Parameters
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut buf = std::io::Cursor::new(&buf[..]);
            let depacketized_subscribe = Subscribe::decode(&mut buf).unwrap();

            let request_id = 0;
            let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
            let track_name = "track_name".to_string();
            let subscriber_priority = 0;
            let group_order = GroupOrder::Ascending;
            let forward = true;
            let filter_type = FilterType::LatestGroup;
            let expected_subscribe = Subscribe {
                request_id,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                forward,
                filter_type,
                authorization_tokens: vec![],
                delivery_timeout: None,
            };

            assert_eq!(depacketized_subscribe, expected_subscribe);
        }

        #[test]
        fn depacketize_absolute_start() {
            let bytes_array = [
                0, // Subscribe ID (i)
                2, // Track Namespace(tuple): Number of elements
                4, // Track Namespace(b): Length
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
                0,   // Track Request Parameters (..): Number of Parameters
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut buf = std::io::Cursor::new(&buf[..]);
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
            let expected_subscribe = Subscribe {
                request_id,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                forward,
                filter_type,
                authorization_tokens: vec![],
                delivery_timeout: None,
            };

            assert_eq!(depacketized_subscribe, expected_subscribe);
        }

        #[test]
        fn depacketize_absolute_range() {
            let bytes_array = [
                0, // Subscribe ID (i)
                2, // Track Namespace(tuple): Number of elements
                4, // Track Namespace(b): Length
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
                0,   // Track Request Parameters (..): Number of Parameters
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut buf = std::io::Cursor::new(&buf[..]);
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

            let expected_subscribe = Subscribe {
                request_id,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                forward,
                filter_type,
                authorization_tokens: vec![],
                delivery_timeout: None,
            };
            assert_eq!(depacketized_subscribe, expected_subscribe);
        }
    }
}
