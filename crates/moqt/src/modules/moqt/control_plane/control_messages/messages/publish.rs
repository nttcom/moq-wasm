use anyhow::Context;
use bytes::{Buf, BufMut, BytesMut};

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt},
    moqt::control_plane::control_messages::{
        key_value_pair::{KeyValuePair, find_even, odd_values},
        messages::parameters::{
            authorization_token::AuthorizationToken, content_exists::ContentExists,
            group_order::GroupOrder,
        },
        util,
    },
};

#[derive(Debug, PartialEq)]
pub struct Publish {
    pub request_id: u64,
    pub track_namespace_tuple: Vec<String>,
    pub track_name: String,
    pub track_alias: u64,
    pub group_order: GroupOrder,
    pub content_exists: ContentExists,
    pub forward: bool,
    pub authorization_tokens: Vec<AuthorizationToken>,
    pub delivery_timeout: Option<u64>,
    pub max_duration: Option<u64>,
}

impl Publish {
    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<Self> {
        let request_id = buf.try_get_varint().context("request id")?;
        let track_namespace_tuple = buf.try_get_tuple().context("track namespace")?;
        let track_name = buf.try_get_string().context("track name")?;
        let track_alias = buf.try_get_varint().context("track alias")?;
        let group_order_u8 = buf.try_get_u8().context("group order u8")?;
        let group_order = GroupOrder::try_from(group_order_u8).context("group order")?;
        let content_exists = ContentExists::decode(buf)?;

        let forward_u8 = buf.try_get_u8().context("forward u8")?;
        let forward = util::u8_to_bool(forward_u8).context("forward")?;
        let parameters = KeyValuePair::decode_list(buf)?;
        let authorization_tokens = odd_values(&parameters, 0x03)
            .filter_map(|value| AuthorizationToken::decode_parameter(value))
            .collect();
        let delivery_timeout = find_even(&parameters, 0x02);
        let max_duration = find_even(&parameters, 0x04);
        Ok(Self {
            request_id,
            track_namespace_tuple,
            track_name,
            track_alias,
            group_order,
            content_exists,
            forward,
            authorization_tokens,
            delivery_timeout,
            max_duration,
        })
    }

    pub fn encode(&self) -> bytes::BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_tuple(&self.track_namespace_tuple);
        payload.put_string(&self.track_name);
        payload.put_varint(self.track_alias);
        payload.put_u8(self.group_order as u8);
        payload.unsplit(self.content_exists.encode());
        payload.put_u8(self.forward as u8);

        let parameters: Vec<KeyValuePair> = self
            .authorization_tokens
            .iter()
            .map(|token| KeyValuePair::odd(0x03, token.encode().freeze()))
            .chain(
                [
                    self.delivery_timeout
                        .map(|value| KeyValuePair::even(0x02, value)),
                    self.max_duration
                        .map(|value| KeyValuePair::even(0x04, value)),
                ]
                .into_iter()
                .flatten(),
            )
            .collect();
        payload.unsplit(KeyValuePair::encode_list(&parameters));

        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use bytes::Bytes;
        use crate::modules::moqt::control_plane::control_messages::messages::parameters::content_exists::ContentExists;
        use crate::modules::moqt::control_plane::control_messages::messages::parameters::group_order::GroupOrder;
        use crate::modules::moqt::control_plane::control_messages::messages::{
            parameters::{authorization_token::AuthorizationToken, location::Location},
            publish::Publish,
        };

        #[test]
        fn packetize_and_depacketize_with_location_and_params() {
            let publish_message = Publish {
                request_id: 1,
                track_namespace_tuple: vec!["moq".to_string(), "news".to_string()],
                track_name: "video".to_string(),
                track_alias: 2,
                group_order: GroupOrder::Ascending, // Ascending
                content_exists: ContentExists::True {
                    location: Location {
                        group_id: 10,
                        object_id: 5,
                    },
                },
                forward: true,
                authorization_tokens: vec![],
                delivery_timeout: None,
                max_duration: None,
            };

            let buf = publish_message.encode();
            let mut buf = std::io::Cursor::new(&buf[..]);

            // depacketize
            let depacketized_message = Publish::decode(&mut buf).unwrap();

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
            assert_eq!(publish_message.forward, depacketized_message.forward);
            assert!(publish_message.authorization_tokens.is_empty());
            assert_eq!(
                publish_message.delivery_timeout,
                depacketized_message.delivery_timeout
            );
            assert_eq!(
                publish_message.max_duration,
                depacketized_message.max_duration
            );
        }

        #[test]
        fn packetize_and_depacketize_without_location_or_params() {
            let publish_message = Publish {
                request_id: 1,
                track_namespace_tuple: vec!["moq".to_string()],
                track_name: "audio".to_string(),
                track_alias: 3,
                group_order: GroupOrder::Descending, // Descending
                content_exists: ContentExists::False,
                forward: false,
                authorization_tokens: vec![],
                delivery_timeout: None,
                max_duration: None,
            };

            let buf = publish_message.encode();
            let mut buf = std::io::Cursor::new(&buf[..]);

            // depacketize
            let depacketized_message = Publish::decode(&mut buf).unwrap();

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
            assert_eq!(publish_message.forward, depacketized_message.forward);
            assert!(publish_message.authorization_tokens.is_empty());
            assert_eq!(
                publish_message.delivery_timeout,
                depacketized_message.delivery_timeout
            );
            assert_eq!(
                publish_message.max_duration,
                depacketized_message.max_duration
            );
        }

        #[test]
        fn packetize_check_bytes() {
            let publish_message = Publish {
                request_id: 1,
                track_namespace_tuple: vec!["moq".to_string()],
                track_name: "video".to_string(),
                track_alias: 2,
                group_order: GroupOrder::Ascending,
                content_exists: ContentExists::False,
                forward: true,
                authorization_tokens: vec![],
                delivery_timeout: None,
                max_duration: None,
            };

            let buf = publish_message.encode();

            let expected_bytes = vec![
                1, 1, 3, b'm', b'o', b'q', 5, b'v', b'i', b'd', b'e', b'o', 2, 1, 0, 1, 0,
            ];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }

        #[test]
        fn packetize_and_depacketize_with_authorization_token() {
            let publish_message = Publish {
                request_id: 1,
                track_namespace_tuple: vec!["moq".to_string()],
                track_name: "video".to_string(),
                track_alias: 2,
                group_order: GroupOrder::Ascending,
                content_exists: ContentExists::False,
                forward: true,
                authorization_tokens: vec![AuthorizationToken::UseValue {
                    token_type: 0,
                    token_value: Bytes::from_static(b"secret"),
                }],
                delivery_timeout: None,
                max_duration: None,
            };

            let buf = publish_message.encode();
            let mut buf = std::io::Cursor::new(&buf[..]);
            let depacketized_message = Publish::decode(&mut buf).unwrap();

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
            assert_eq!(publish_message.forward, depacketized_message.forward);
            assert_eq!(
                publish_message.authorization_tokens,
                depacketized_message.authorization_tokens
            );
            assert_eq!(
                publish_message.delivery_timeout,
                depacketized_message.delivery_timeout
            );
            assert_eq!(
                publish_message.max_duration,
                depacketized_message.max_duration
            );
        }
    }
}
