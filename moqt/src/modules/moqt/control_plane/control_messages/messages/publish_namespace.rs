use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::{
        message_parameters::MessageParameters,
        messages::parameters::authorization_token::AuthorizationToken,
    },
};
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

    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        let track_namespace_tuple_length = buf
            .try_get_varint()
            .log_context("track namespace tuple length")
            .ok()?;
        let mut track_namespace_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let track_namespace = buf.try_get_string().log_context("track namespace").ok()?;
            track_namespace_tuple.push(track_namespace);
        }
        let MessageParameters {
            authorization_tokens: authorization_token,
            ..
        } = MessageParameters::decode(buf)?;

        let announce_message = PublishNamespace {
            request_id,
            track_namespace: track_namespace_tuple,
            authorization_token,
        };

        Some(announce_message)
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        let track_namespace_tuple_length = self.track_namespace.len();
        payload.put_varint(track_namespace_tuple_length as u64);
        self.track_namespace
            .iter()
            .for_each(|track_namespace| payload.put_string(track_namespace));
        payload.unsplit(
            MessageParameters {
                authorization_tokens: self.authorization_token.clone(),
                ..Default::default()
            }
            .encode(),
        );

        tracing::trace!("Packetized Announce message.");
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {

        mod packetize {
            use crate::modules::moqt::control_plane::control_messages::messages::{
                parameters::authorization_token::AuthorizationToken,
                publish_namespace::PublishNamespace,
            };

            #[test]
            fn with_parameter() {
                // Arrange
                let announce_message = PublishNamespace::new(
                    0,
                    Vec::from(["test".to_string()]),
                    vec![AuthorizationToken::use_value_utf8("jwt")],
                );

                // Act
                let buf = announce_message.encode();

                // Assert
                let expected_bytes_array = [
                    0, // request id(u64)
                    1, // Track Namespace(tuple): Number of elements
                    4, // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    1,   // Number of Parameters (i)
                    3,   // Parameter Type (i): AUTHORIZATION TOKEN
                    5,   // Parameter Length (i)
                    3,   // Alias Type (i): USE_VALUE
                    0,   // Token Type (i)
                    106, 119, 116, // Token Value: "jwt"
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

        mod depacketize {
            use crate::modules::moqt::control_plane::control_messages::messages::{
                parameters::authorization_token::AuthorizationToken,
                publish_namespace::PublishNamespace,
            };
            use bytes::BytesMut;
            #[test]
            fn with_parameter() {
                // Arrange
                let bytes_array = [
                    0, // request id(u64)
                    1, // Track Namespace(tuple): Number of elements
                    4, // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    1,   // Number of Parameters (i)
                    3,   // Parameter Type (i): AUTHORIZATION TOKEN
                    5,   // Parameter Length (i)
                    3,   // Alias Type (i): USE_VALUE
                    0,   // Token Type (i)
                    106, 119, 116, // Token Value: "jwt"
                ];
                let mut buf = std::io::Cursor::new(&bytes_array[..]);

                // Act
                let depacketized_announce_message = PublishNamespace::decode(&mut buf).unwrap();

                // Assert
                let expected_announce_message = PublishNamespace::new(
                    0,
                    Vec::from(["test".to_string()]),
                    vec![AuthorizationToken::use_value_utf8("jwt")],
                );
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
