use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::{
        message_parameters::MessageParameters,
        messages::parameters::authorization_token::AuthorizationToken,
    },
};
use bytes::BytesMut;

#[derive(Debug, Clone, PartialEq)]
pub struct SubscribeNamespace {
    pub request_id: u64,
    pub track_namespace_prefix: Vec<String>,
    pub authorization_token: Vec<AuthorizationToken>,
}

impl SubscribeNamespace {
    pub fn new(
        request_id: u64,
        track_namespace_prefix: Vec<String>,
        authorization_token: Vec<AuthorizationToken>,
    ) -> Self {
        SubscribeNamespace {
            request_id,
            track_namespace_prefix,
            authorization_token,
        }
    }
}

impl SubscribeNamespace {
    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        let track_namespace_prefix_tuple_length = buf
            .try_get_varint()
            .log_context("track namespace prefix length")
            .ok()?;
        let mut track_namespace_prefix_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_prefix_tuple_length {
            let track_namespace_prefix = buf
                .try_get_string()
                .log_context("track namespace prefix")
                .ok()?;
            track_namespace_prefix_tuple.push(track_namespace_prefix);
        }

        let MessageParameters {
            authorization_tokens,
            ..
        } = MessageParameters::decode(buf)?;

        Some(SubscribeNamespace {
            request_id,
            track_namespace_prefix: track_namespace_prefix_tuple,
            authorization_token: authorization_tokens,
        })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_varint(self.track_namespace_prefix.len() as u64);
        self.track_namespace_prefix
            .iter()
            .for_each(|track_namespace_prefix| {
                payload.put_string(track_namespace_prefix);
            });
        payload.unsplit(
            MessageParameters {
                authorization_tokens: self.authorization_token.clone(),
                ..Default::default()
            }
            .encode(),
        );
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::control_plane::control_messages::messages::{
            parameters::authorization_token::AuthorizationToken,
            subscribe_namespace::SubscribeNamespace,
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let request_id = 0;
            let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
            let authorization_tokens = vec![];
            let subscribe_announces = SubscribeNamespace::new(
                request_id,
                track_namespace_prefix.clone(),
                authorization_tokens,
            );
            let buf = subscribe_announces.encode();

            let expected_bytes_array = [
                0, // Request ID(i)
                2, // Track Namespace Prefix(tuple): Number of elements
                4, // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                4,   // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                0,   // Parameters (..): Number of Parameters
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize() {
            let bytes_array = [
                0, // Request ID(i)
                2, // Track Namespace Prefix(tuple): Number of elements
                4, // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                4,   // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                0,   // Parameters (..): Number of Parameters
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut cursor = std::io::Cursor::new(buf.as_ref());
            let subscribe_announces = SubscribeNamespace::decode(&mut cursor).unwrap();

            let request_id = 0;
            let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
            let parameters = vec![];
            let expected_subscribe_announces =
                SubscribeNamespace::new(request_id, track_namespace_prefix, parameters);

            assert_eq!(subscribe_announces, expected_subscribe_announces);
        }

        #[test]
        fn authorization_tokens_round_trip_as_parameters() {
            // Arrange
            let message = SubscribeNamespace::new(
                0,
                vec!["test".to_string()],
                vec![
                    AuthorizationToken::use_value_utf8("a"),
                    AuthorizationToken::use_value_utf8("b"),
                ],
            );

            // Act
            let buf = message.encode();
            let decoded =
                SubscribeNamespace::decode(&mut std::io::Cursor::new(buf.as_ref())).unwrap();

            // Assert
            assert_eq!(decoded, message);
        }
    }
}
