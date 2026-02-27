use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::{
        key_value_pair::{KeyValuePair, VariantType},
        messages::parameters::{
            content_exists::ContentExists, group_order::GroupOrder,
        },
    },
};
use bytes::{Buf, BufMut, BytesMut};

#[derive(Debug, PartialEq, Clone)]
pub struct SubscribeOk {
    pub(crate) request_id: u64,
    pub(crate) track_alias: u64,
    pub(crate) expires: u64,
    pub(crate) group_order: GroupOrder,
    pub(crate) content_exists: ContentExists,
    pub(crate) delivery_timeout: Option<u64>,
    pub(crate) max_duration: Option<u64>,
}

impl SubscribeOk {
    pub(crate) fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        let track_alias = buf.try_get_varint().log_context("track alias").ok()?;
        let expires = buf.try_get_varint().log_context("expires").ok()?;
        let group_order_u8 = buf.try_get_u8().log_context("group order u8").ok()?;

        // Values larger than 0x2 are a Protocol Violation.
        let group_order = GroupOrder::try_from(group_order_u8)
            .log_context("group order")
            .ok()?;

        let content_exists = ContentExists::decode(buf)?;

        let number_of_parameters = buf
            .try_get_varint()
            .log_context("number of parameters")
            .ok()?;
        let mut parameters = vec![];
        for _ in 0..number_of_parameters {
            let params = KeyValuePair::decode(buf)?;
            parameters.push(params);
        }
        let delivery_timeout =
            parameters
                .iter()
                .find(|kv_pair| kv_pair.key == 0x02)
                .map(|kv_pair| match kv_pair.value {
                    VariantType::Odd(_) => unreachable!(),
                    VariantType::Even(value) => value,
                });
        let max_duration = parameters
            .iter()
            .find(|kv_pair| kv_pair.key == 0x04)
            .map(|kv_pair| match kv_pair.value {
                VariantType::Odd(_) => unreachable!(),
                VariantType::Even(value) => value,
            });

        Some(SubscribeOk {
            request_id,
            track_alias,
            expires,
            group_order,
            content_exists,
            delivery_timeout,
            max_duration,
        })
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_varint(self.track_alias);
        payload.put_varint(self.expires);
        payload.put_u8(self.group_order as u8);
        payload.unsplit(self.content_exists.encode());
        let mut number_of_parameters = 0;
        let mut parameters_payload = BytesMut::new();
        if let Some(delivery_timeout) = self.delivery_timeout {
            let delivery_timeout_payload = KeyValuePair {
                key: 0x02,
                value: VariantType::Even(delivery_timeout),
            }
            .encode();
            parameters_payload.unsplit(delivery_timeout_payload);
            number_of_parameters += 1;
        }
        if let Some(max_duration) = self.max_duration {
            let max_duration_payload = KeyValuePair {
                key: 0x04,
                value: VariantType::Even(max_duration),
            }
            .encode();
            parameters_payload.unsplit(max_duration_payload);
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
        use crate::modules::moqt::control_plane::control_messages::messages::{
            parameters::{
                content_exists::ContentExists,
                group_order::GroupOrder,
                location::Location,
            },
            subscribe_ok::SubscribeOk,
        };
        use bytes::BytesMut;

        #[test]
        fn packetize_content_not_exists() {
            let request_id = 0;
            let track_alias = 1;
            let expires = 1;
            let group_order = GroupOrder::Ascending;
            let content_exists = ContentExists::False;

            let subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                delivery_timeout: None,
                max_duration: None,
            };
            let buf = subscribe_ok.encode();

            let expected_bytes_array = [
                0, // Request ID (i)
                1, // Track alias (i)
                1, // Expires (i)
                1, // Group Order (8)
                0, // Content Exists (f)
                0, // Track Request Parameters (..): Number of Parameters
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn packetize_content_exists() {
            let request_id = 0;
            let track_alias = 2;
            let expires = 1;
            let group_order = GroupOrder::Descending;
            let content_exists = ContentExists::True {
                location: Location {
                    group_id: 10,
                    object_id: 20,
                },
            };

            let subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                delivery_timeout: None,
                max_duration: None,
            };
            let buf = subscribe_ok.encode();

            let expected_bytes_array = [
                0,  // Request ID (i)
                2,  // Track alias (i)
                1,  // Expires (i)
                2,  // Group Order (8)
                1,  // Content Exists (f)
                10, // Largest Group ID (i)
                20, // Largest Object ID (i)
                0,  // Track Request Parameters (..): Number of Parameters
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize_content_not_exists() {
            let bytes_array = [
                0, // Request ID (i)
                1, // Track alias (i)
                1, // Expires (i)
                2, // Group Order (8)
                0, // Content Exists (f)
                0, // Track Request Parameters (..): Number of Parameters
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut buf = std::io::Cursor::new(&buf[..]);
            let depacketized_subscribe_ok = SubscribeOk::decode(&mut buf).unwrap();

            let request_id = 0;
            let track_alias = 1;
            let expires = 1;
            let group_order = GroupOrder::Descending;
            let content_exists = ContentExists::False;

            let expected_subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                delivery_timeout: None,
                max_duration: None,
            };

            assert_eq!(depacketized_subscribe_ok, expected_subscribe_ok);
        }

        #[test]
        fn depacketize_content_exists() {
            let bytes_array = [
                0,  // Request ID (i)
                2,  // Track alias (i)
                1,  // Expires (i)
                1,  // Group Order (8)
                1,  // Content Exists (f)
                0,  // Largest Group ID (i)
                5,  // Largest Object ID (i)
                0,  // Track Request Parameters (..): Number of Parameters
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut buf = std::io::Cursor::new(&buf[..]);
            let depacketized_subscribe_ok = SubscribeOk::decode(&mut buf).unwrap();

            let request_id = 0;
            let track_alias = 2;
            let expires = 1;
            let group_order = GroupOrder::Ascending;
            let content_exists = ContentExists::True {
                location: Location {
                    group_id: 0,
                    object_id: 5,
                },
            };

            let expected_subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                delivery_timeout: None,
                max_duration: None,
            };

            assert_eq!(depacketized_subscribe_ok, expected_subscribe_ok);
        }
    }
}
