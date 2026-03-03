use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::messages::parameters::authorization_token::AuthorizationToken,
};
use bytes::BytesMut;

#[derive(Debug, Clone, PartialEq)]
pub struct PublishNamespace {
    pub(crate) request_id: u64,
    pub(crate) track_namespace: Vec<String>,
    pub(crate) number_of_parameters: u64,
    pub authorization_token: Vec<AuthorizationToken>,
}

impl PublishNamespace {
    pub fn new(
        request_id: u64,
        track_namespace: Vec<String>,
        authorization_token: Vec<AuthorizationToken>,
    ) -> Self {
        let number_of_parameters = authorization_token.len() as u64;
        PublishNamespace {
            request_id,
            track_namespace,
            number_of_parameters,
            authorization_token,
        }
    }

    pub(crate) fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
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
        let number_of_parameters = buf
            .try_get_varint()
            .log_context("number of parameters")
            .ok()?;
        let mut authorization_token = vec![];
        for _ in 0..number_of_parameters {
            let token = AuthorizationToken::decode(buf)?;
            authorization_token.push(token);
        }

        let announce_message = PublishNamespace {
            request_id,
            track_namespace: track_namespace_tuple,
            number_of_parameters,
            authorization_token,
        };

        Some(announce_message)
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        let track_namespace_tuple_length = self.track_namespace.len();
        payload.put_varint(track_namespace_tuple_length as u64);
        self.track_namespace
            .iter()
            .for_each(|track_namespace| payload.put_string(track_namespace));
        payload.put_varint(self.number_of_parameters);
        // Parameters
        for param in &self.authorization_token {
            let param = param.encode();
            payload.unsplit(param);
        }

        tracing::trace!("Packetized Announce message.");
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {

        mod packetize {
            use crate::modules::moqt::control_plane::control_messages::messages::publish_namespace::PublishNamespace;

            // TODO: Add parameter of AuthorizationToken type
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

        mod depacketize {
            use crate::modules::moqt::control_plane::control_messages::messages::publish_namespace::PublishNamespace;
            use bytes::BytesMut;
            #[test]
            fn with_parameter() {
                let bytes_array = [
                    0,  // request id(u64)
                    2,  // Track Namespace(tuple): Number of elements
                    4,  // Track Namespace(b): Length
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
                    0,  // request id(u64)
                    2,  // Track Namespace(tuple): Number of elements
                    4,  // Track Namespace(b): Length
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
