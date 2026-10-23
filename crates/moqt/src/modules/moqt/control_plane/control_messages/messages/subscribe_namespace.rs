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

    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<Self> {
        let request_id = buf.try_get_varint().context("request id")?;
        let track_namespace_prefix = buf.try_get_tuple().context("track namespace prefix")?;
        let parameters = KeyValuePair::decode_list(buf)?;
        let authorization_token = odd_values(&parameters, 0x03)
            .filter_map(|value| AuthorizationToken::decode_parameter(value))
            .collect();

        Ok(Self {
            request_id,
            track_namespace_prefix,
            authorization_token,
        })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_tuple(&self.track_namespace_prefix);
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
