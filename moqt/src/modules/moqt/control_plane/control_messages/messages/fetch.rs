use bytes::{Buf, BufMut, BytesMut};
use num_enum::{IntoPrimitive, TryFromPrimitive};

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::{
        key_value_pair::{KeyValuePair, VariantType},
        messages::parameters::{
            authorization_token::AuthorizationToken, group_order::GroupOrder, location::Location,
        },
    },
};

// https://www.ietf.org/archive/id/draft-ietf-moq-transport-14.html#section-9.16
//
// Standalone Fetch {
//   Track Namespace (tuple),
//   Track Name Length (i),
//   Track Name (..),
//   Start Location (Location),
//   End Location (Location)
// }
//
// Joining Fetch {
//   Joining Request ID (i),
//   Joining Start (i)
// }

#[derive(Debug, Clone, TryFromPrimitive, IntoPrimitive)]
#[repr(u64)]
enum FetchTypeValue {
    Standalone = 0x1,
    RelativeJoining = 0x2,
    AbsoluteJoining = 0x3,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FetchType {
    Standalone {
        track_namespace: Vec<String>,
        track_name: String,
        start_location: Location,
        end_location: Location,
    },
    RelativeJoining {
        joining_request_id: u64,
        joining_start: u64,
    },
    AbsoluteJoining {
        joining_request_id: u64,
        joining_start: u64,
    },
}

impl FetchType {
    fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let fetch_type_value = buf.try_get_varint().log_context("fetch type").ok()?;
        let fetch_type = FetchTypeValue::try_from(fetch_type_value)
            .log_context("fetch type value")
            .ok()?;
        match fetch_type {
            FetchTypeValue::Standalone => {
                let namespace_length = buf.try_get_varint().log_context("namespace length").ok()?;
                let mut track_namespace = Vec::new();
                for _ in 0..namespace_length {
                    let ns = buf.try_get_string().log_context("namespace").ok()?;
                    track_namespace.push(ns);
                }
                let track_name = buf.try_get_string().log_context("track name").ok()?;
                let start_location = Location::decode(buf)?;
                let end_location = Location::decode(buf)?;
                Some(FetchType::Standalone {
                    track_namespace,
                    track_name,
                    start_location,
                    end_location,
                })
            }
            FetchTypeValue::RelativeJoining => {
                let joining_request_id = buf
                    .try_get_varint()
                    .log_context("joining request id")
                    .ok()?;
                let joining_start = buf.try_get_varint().log_context("joining start").ok()?;
                Some(FetchType::RelativeJoining {
                    joining_request_id,
                    joining_start,
                })
            }
            FetchTypeValue::AbsoluteJoining => {
                let joining_request_id = buf
                    .try_get_varint()
                    .log_context("joining request id")
                    .ok()?;
                let joining_start = buf.try_get_varint().log_context("joining start").ok()?;
                Some(FetchType::AbsoluteJoining {
                    joining_request_id,
                    joining_start,
                })
            }
        }
    }

    fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        match self {
            FetchType::Standalone {
                track_namespace,
                track_name,
                start_location,
                end_location,
            } => {
                payload.put_varint(FetchTypeValue::Standalone as u64);
                payload.put_varint(track_namespace.len() as u64);
                for ns in track_namespace {
                    payload.put_string(ns);
                }
                payload.put_string(track_name);
                payload.unsplit(start_location.encode());
                payload.unsplit(end_location.encode());
            }
            FetchType::RelativeJoining {
                joining_request_id,
                joining_start,
            } => {
                payload.put_varint(FetchTypeValue::RelativeJoining as u64);
                payload.put_varint(*joining_request_id);
                payload.put_varint(*joining_start);
            }
            FetchType::AbsoluteJoining {
                joining_request_id,
                joining_start,
            } => {
                payload.put_varint(FetchTypeValue::AbsoluteJoining as u64);
                payload.put_varint(*joining_request_id);
                payload.put_varint(*joining_start);
            }
        }
        payload
    }
}

// https://www.ietf.org/archive/id/draft-ietf-moq-transport-14.html#section-9.16
//
// FETCH Message {
//   Type (i) = 0x16,
//   Length (16),
//   Request ID (i),
//   Subscriber Priority (8),
//   Group Order (8),
//   Fetch Type (i),
//   [Standalone (Standalone Fetch)],
//   [Joining (Joining Fetch)],
//   Number of Parameters (i),
//   Parameters (..) ...
// }

#[derive(Debug, Clone, PartialEq)]
pub struct Fetch {
    pub request_id: u64,
    pub subscriber_priority: u8,
    pub group_order: GroupOrder,
    pub fetch_type: FetchType,
    pub authorization_tokens: Vec<AuthorizationToken>,
}

impl Fetch {
    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        let subscriber_priority = buf.try_get_u8().log_context("subscriber priority").ok()?;
        let group_order_u8 = buf.try_get_u8().log_context("group order u8").ok()?;
        let group_order = GroupOrder::try_from(group_order_u8)
            .log_context("group order")
            .ok()?;
        let fetch_type = FetchType::decode(buf)?;
        let number_of_parameters = buf
            .try_get_varint()
            .log_context("number of parameters")
            .ok()?;
        let mut parameters = vec![];
        for _ in 0..number_of_parameters {
            let kv = KeyValuePair::decode(buf)?;
            parameters.push(kv);
        }
        let authorization_tokens = parameters
            .iter()
            .filter(|kv| kv.key == 0x03)
            .filter_map(|kv| match &kv.value {
                VariantType::Odd(value) => {
                    let mut cursor = std::io::Cursor::new(&value[..]);
                    AuthorizationToken::decode(&mut cursor)
                }
                VariantType::Even(_) => unreachable!(),
            })
            .collect();
        Some(Fetch {
            request_id,
            subscriber_priority,
            group_order,
            fetch_type,
            authorization_tokens,
        })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_u8(self.subscriber_priority);
        payload.put_u8(self.group_order as u8);
        payload.unsplit(self.fetch_type.encode());
        let mut number_of_parameters = 0;
        let mut parameters_payload = BytesMut::new();
        for token in &self.authorization_tokens {
            let token_payload = KeyValuePair {
                key: 0x03,
                value: VariantType::Odd(token.encode().freeze()),
            }
            .encode();
            parameters_payload.unsplit(token_payload);
            number_of_parameters += 1;
        }
        payload.put_varint(number_of_parameters);
        payload.unsplit(parameters_payload);
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use super::super::*;

        #[test]
        fn standalone_round_trip() {
            let fetch = Fetch {
                request_id: 1,
                subscriber_priority: 0,
                group_order: GroupOrder::Ascending,
                fetch_type: FetchType::Standalone {
                    track_namespace: vec!["test".to_string(), "ns".to_string()],
                    track_name: "track".to_string(),
                    start_location: Location {
                        group_id: 0,
                        object_id: 0,
                    },
                    end_location: Location {
                        group_id: 10,
                        object_id: 0,
                    },
                },
                authorization_tokens: vec![],
            };
            let buf = fetch.encode();
            let mut cursor = std::io::Cursor::new(&buf[..]);
            let result = Fetch::decode(&mut cursor).unwrap();
            assert_eq!(result, fetch);
        }

        #[test]
        fn standalone_packetize() {
            let fetch = Fetch {
                request_id: 1,
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                fetch_type: FetchType::Standalone {
                    track_namespace: vec!["a".to_string()],
                    track_name: "b".to_string(),
                    start_location: Location {
                        group_id: 0,
                        object_id: 0,
                    },
                    end_location: Location {
                        group_id: 5,
                        object_id: 0,
                    },
                },
                authorization_tokens: vec![],
            };
            let result = fetch.encode();
            let expected = [
                1,   // Request ID
                128, // Subscriber Priority
                1,   // Group Order = Ascending
                1,   // Fetch Type = Standalone
                1,   // Track Namespace: number of elements
                1, b'a', // Track Namespace[0]: length + "a"
                1, b'b', // Track Name: length + "b"
                0,    // Start Location: group_id
                0,    // Start Location: object_id
                5,    // End Location: group_id
                0,    // End Location: object_id
                0,    // Number of Parameters
            ];
            assert_eq!(result.as_ref(), expected.as_slice());
        }

        #[test]
        fn standalone_depacketize() {
            let bytes = [
                1,   // Request ID
                128, // Subscriber Priority
                1,   // Group Order = Ascending
                1,   // Fetch Type = Standalone
                1,   // Track Namespace: number of elements
                1, b'a', // Track Namespace[0]: length + "a"
                1, b'b', // Track Name: length + "b"
                0,    // Start Location: group_id
                0,    // Start Location: object_id
                5,    // End Location: group_id
                0,    // End Location: object_id
                0,    // Number of Parameters
            ];
            let mut cursor = std::io::Cursor::new(&bytes[..]);
            let result = Fetch::decode(&mut cursor).unwrap();
            let expected = Fetch {
                request_id: 1,
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                fetch_type: FetchType::Standalone {
                    track_namespace: vec!["a".to_string()],
                    track_name: "b".to_string(),
                    start_location: Location {
                        group_id: 0,
                        object_id: 0,
                    },
                    end_location: Location {
                        group_id: 5,
                        object_id: 0,
                    },
                },
                authorization_tokens: vec![],
            };
            assert_eq!(result, expected);
        }

        #[test]
        fn relative_joining_packetize() {
            let fetch = Fetch {
                request_id: 5,
                subscriber_priority: 0,
                group_order: GroupOrder::Descending,
                fetch_type: FetchType::RelativeJoining {
                    joining_request_id: 3,
                    joining_start: 2,
                },
                authorization_tokens: vec![],
            };
            let result = fetch.encode();
            let expected = [
                5, // Request ID
                0, // Subscriber Priority
                2, // Group Order = Descending
                2, // Fetch Type = RelativeJoining
                3, // Joining Request ID
                2, // Joining Start
                0, // Number of Parameters
            ];
            assert_eq!(result.as_ref(), expected.as_slice());
        }

        #[test]
        fn relative_joining_depacketize() {
            let bytes = [
                5, // Request ID
                0, // Subscriber Priority
                2, // Group Order = Descending
                2, // Fetch Type = RelativeJoining
                3, // Joining Request ID
                2, // Joining Start
                0, // Number of Parameters
            ];
            let mut cursor = std::io::Cursor::new(&bytes[..]);
            let result = Fetch::decode(&mut cursor).unwrap();
            let expected = Fetch {
                request_id: 5,
                subscriber_priority: 0,
                group_order: GroupOrder::Descending,
                fetch_type: FetchType::RelativeJoining {
                    joining_request_id: 3,
                    joining_start: 2,
                },
                authorization_tokens: vec![],
            };
            assert_eq!(result, expected);
        }

        #[test]
        fn absolute_joining_packetize() {
            let fetch = Fetch {
                request_id: 7,
                subscriber_priority: 0,
                group_order: GroupOrder::Ascending,
                fetch_type: FetchType::AbsoluteJoining {
                    joining_request_id: 4,
                    joining_start: 100,
                },
                authorization_tokens: vec![],
            };
            let result = fetch.encode();
            let expected = [
                7, // Request ID
                0, // Subscriber Priority
                1, // Group Order = Ascending
                3, // Fetch Type = AbsoluteJoining
                4, // Joining Request ID
                64, 100, // Joining Start (100 as 2-byte varint)
                0,   // Number of Parameters
            ];
            assert_eq!(result.as_ref(), expected.as_slice());
        }

        #[test]
        fn absolute_joining_depacketize() {
            let bytes = [
                7, // Request ID
                0, // Subscriber Priority
                1, // Group Order = Ascending
                3, // Fetch Type = AbsoluteJoining
                4, // Joining Request ID
                64, 100, // Joining Start (100 as 2-byte varint)
                0,   // Number of Parameters
            ];
            let mut cursor = std::io::Cursor::new(&bytes[..]);
            let result = Fetch::decode(&mut cursor).unwrap();
            let expected = Fetch {
                request_id: 7,
                subscriber_priority: 0,
                group_order: GroupOrder::Ascending,
                fetch_type: FetchType::AbsoluteJoining {
                    joining_request_id: 4,
                    joining_start: 100,
                },
                authorization_tokens: vec![],
            };
            assert_eq!(result, expected);
        }
    }

    mod failure {
        use super::super::*;

        #[test]
        fn invalid_fetch_type() {
            let bytes = [
                1, // Request ID
                0, // Subscriber Priority
                1, // Group Order = Ascending
                4, // Fetch Type = 0x4 (invalid)
            ];
            let mut cursor = std::io::Cursor::new(&bytes[..]);
            assert!(Fetch::decode(&mut cursor).is_none());
        }

        #[test]
        fn invalid_group_order() {
            let bytes = [
                1, // Request ID
                0, // Subscriber Priority
                3, // Group Order = 0x3 (invalid)
            ];
            let mut cursor = std::io::Cursor::new(&bytes[..]);
            assert!(Fetch::decode(&mut cursor).is_none());
        }
    }
}
