use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt},
    moqt::control_plane::control_messages::{
        key_value_pair::{KeyValuePair, odd_values},
        messages::parameters::authorization_token::AuthorizationToken,
    },
};
use anyhow::Context;
use bytes::BytesMut;

#[derive(Debug, Clone, PartialEq)]
pub struct PublishNamespace {
    pub request_id: u64,
    pub track_namespace: Vec<String>,
    pub authorization_token: Vec<AuthorizationToken>,
}

impl PublishNamespace {
    pub fn new(
        request_id: u64,
        track_namespace: Vec<String>,
        authorization_token: Vec<AuthorizationToken>,
    ) -> Self {
        PublishNamespace {
            request_id,
            track_namespace,
            authorization_token,
        }
    }

    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<Self> {
        let request_id = buf.try_get_varint().context("request id")?;
        let track_namespace = buf.try_get_tuple().context("track namespace")?;
        let parameters = KeyValuePair::decode_list(buf)?;
        let authorization_token = odd_values(&parameters, 0x03)
            .filter_map(|value| AuthorizationToken::decode_parameter(value))
            .collect();

        Ok(Self {
            request_id,
            track_namespace,
            authorization_token,
        })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_tuple(&self.track_namespace);
        let parameters: Vec<KeyValuePair> = self
            .authorization_token
            .iter()
            .map(|token| KeyValuePair::odd(0x03, token.encode().freeze()))
            .collect();
        payload.unsplit(KeyValuePair::encode_list(&parameters));
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {

        mod packetize {
            use crate::modules::moqt::control_plane::control_messages::messages::publish_namespace::PublishNamespace;

            #[test]
            fn with_parameter() {
                let request_id = 0;
                let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
                let parameters = vec![];
                let announce_message =
                    PublishNamespace::new(request_id, track_namespace.clone(), parameters);
                let buf = announce_message.encode();

                let expected_bytes_array = [
                    0, // request id(u64)
                    2, // Track Namespace(tuple): Number of elements
                    4, // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    4,   // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    0,   // Number of Parameters (i)
                ];

                assert_eq!(buf.as_ref(), expected_bytes_array);
            }
            #[test]
            fn without_parameter() {
                let request_id = 0;
                let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
                let parameters = vec![];
                let announce_message =
                    PublishNamespace::new(request_id, track_namespace.clone(), parameters);
                let buf = announce_message.encode();

                let expected_bytes_array = [
                    0, // request id(u64)
                    2, // Track Namespace(tuple): Number of elements
                    4, // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    4,   // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    0,   // Number of Parameters (i)
                ];

                assert_eq!(buf.as_ref(), expected_bytes_array);
            }
        }

        #[test]
        fn frames_an_authorization_token_as_a_key_value_pair() {
            use crate::modules::moqt::control_plane::control_messages::messages::{
                parameters::authorization_token::AuthorizationToken,
                publish_namespace::PublishNamespace,
            };

            // Arrange
            let message = PublishNamespace::new(
                1,
                vec!["room".to_string()],
                vec![AuthorizationToken::use_value_utf8("jwt")],
            );
            let expected_bytes = [
                1, // Request ID (i)
                1, // Track Namespace(tuple): Number of elements
                4, 114, 111, 111, 109, // "room"
                1,   // Number of Parameters (i)
                3,   // Parameter Type: AUTHORIZATION TOKEN
                5,   // Parameter Length
                3,   // Alias Type: USE_VALUE
                0,   // Token Type
                106, 119, 116, // "jwt"
            ];

            // Act
            let bytes = message.encode();
            let decoded = PublishNamespace::decode(&mut std::io::Cursor::new(&bytes[..])).unwrap();

            // Assert
            assert_eq!(bytes.as_ref(), expected_bytes.as_slice());
            assert_eq!(decoded, message);
        }

        mod depacketize {
            use crate::modules::moqt::control_plane::control_messages::messages::publish_namespace::PublishNamespace;
            use bytes::BytesMut;
            #[test]
            fn with_parameter() {
                let bytes_array = [
                    0, // request id(u64)
                    2, // Track Namespace(tuple): Number of elements
                    4, // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    4,   // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    0,   // Number of Parameters (i)
                ];
                let mut buf = BytesMut::with_capacity(bytes_array.len());
                buf.extend_from_slice(&bytes_array);
                let mut buf = std::io::Cursor::new(&buf[..]);
                let depacketized_announce_message = PublishNamespace::decode(&mut buf).unwrap();
                let request_id = 0;
                let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
                let parameters = vec![];
                let expected_announce_message =
                    PublishNamespace::new(request_id, track_namespace.clone(), parameters);

                assert_eq!(depacketized_announce_message, expected_announce_message);
            }

            #[test]
            fn without_parameter() {
                let bytes_array = [
                    0, // request id(u64)
                    2, // Track Namespace(tuple): Number of elements
                    4, // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    4,   // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    0,   // Number of Parameters (i)
                ];
                let mut buf = BytesMut::with_capacity(bytes_array.len());
                buf.extend_from_slice(&bytes_array);
                let mut buf = std::io::Cursor::new(&buf[..]);
                let depacketized_announce_message = PublishNamespace::decode(&mut buf).unwrap();

                let request_id = 0;
                let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
                let parameters = vec![];
                let expected_announce_message =
                    PublishNamespace::new(request_id, track_namespace.clone(), parameters);

                assert_eq!(depacketized_announce_message, expected_announce_message);
            }
        }
    }
}
