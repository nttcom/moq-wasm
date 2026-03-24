use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::messages::parameters::authorization_token::AuthorizationToken,
};
use bytes::BytesMut;

#[derive(Debug, Clone, PartialEq)]
pub struct SubscribeNamespace {
    pub(crate) request_id: u64,
    pub(crate) track_namespace_prefix: Vec<String>,
    number_of_parameters: u64,
    pub authorization_token: Vec<AuthorizationToken>,
}

impl SubscribeNamespace {
    pub fn new(
        request_id: u64,
        track_namespace_prefix: Vec<String>,
        authorization_token: Vec<AuthorizationToken>,
    ) -> Self {
        let number_of_parameters = authorization_token.len() as u64;
        SubscribeNamespace {
            request_id,
            track_namespace_prefix,
            number_of_parameters,
            authorization_token,
        }
    }
}

impl SubscribeNamespace {
    pub(crate) fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
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

        let number_of_parameters = buf
            .try_get_varint()
            .log_context("number of parameters")
            .ok()?;

        let mut authorization_tokens = Vec::new();
        for _ in 0..number_of_parameters {
            let authorization_token = AuthorizationToken::decode(buf)?;
            authorization_tokens.push(authorization_token);
        }

        Some(SubscribeNamespace {
            request_id,
            track_namespace_prefix: track_namespace_prefix_tuple,
            number_of_parameters,
            authorization_token: authorization_tokens,
        })
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_varint(self.track_namespace_prefix.len() as u64);
        self.track_namespace_prefix
            .iter()
            .for_each(|track_namespace_prefix| {
                payload.put_string(track_namespace_prefix);
            });
        payload.put_varint(self.authorization_token.len() as u64);
        for authorization_token in &self.authorization_token {
            let token_payload = authorization_token.encode();
            payload.extend_from_slice(&token_payload);
        }
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::control_plane::control_messages::messages::subscribe_namespace::SubscribeNamespace;
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
    }
}
