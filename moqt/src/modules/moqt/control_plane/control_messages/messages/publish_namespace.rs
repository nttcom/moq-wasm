use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::{
        messages::parameters::version_specific_parameters::VersionSpecificParameter,
        moqt_payload::MOQTPayload, util::add_payload_length,
    },
};
use bytes::BytesMut;
use serde::Serialize;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct PublishNamespace {
    pub(crate) request_id: u64,
    pub(crate) track_namespace: Vec<String>,
    pub(crate) number_of_parameters: u64,
    pub(crate) parameters: Vec<VersionSpecificParameter>,
}

impl PublishNamespace {
    pub fn new(
        request_id: u64,
        track_namespace: Vec<String>,
        parameters: Vec<VersionSpecificParameter>,
    ) -> Self {
        let number_of_parameters = parameters.len() as u64;
        PublishNamespace {
            request_id,
            track_namespace,
            number_of_parameters,
            parameters,
        }
    }

    pub(crate) fn decode(buf: &mut BytesMut) -> Option<Self> {
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
        let mut parameters = vec![];
        for _ in 0..number_of_parameters {
            let version_specific_parameter = VersionSpecificParameter::depacketize(buf).ok()?;
            if let VersionSpecificParameter::Unknown(code) = version_specific_parameter {
                tracing::warn!("unknown track request parameter {}", code);
            } else {
                parameters.push(version_specific_parameter);
            }
        }

        let announce_message = PublishNamespace {
            request_id,
            track_namespace: track_namespace_tuple,
            number_of_parameters,
            parameters,
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
        for param in &self.parameters {
            param.packetize(&mut payload);
        }

        tracing::trace!("Packetized Announce message.");
        add_payload_length(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {

        mod packetize {
            use crate::modules::moqt::control_plane::control_messages::messages::{
                parameters::version_specific_parameters::{
                    AuthorizationInfo, VersionSpecificParameter,
                },
                publish_namespace::PublishNamespace,
            };

            #[test]
            fn with_parameter() {
                let request_id = 0;
                let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);

                let parameter_value = "test".to_string();
                let parameter = VersionSpecificParameter::AuthorizationInfo(
                    AuthorizationInfo::new(parameter_value.clone()),
                );
                let parameters = vec![parameter];
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
                    1,   // Number of Parameters (i)
                    2,   // Parameters (..): Parameter Type(AuthorizationInfo)
                    4,   // Parameters (..): Length
                    116, 101, 115, 116, // Parameters (..): Value("test")
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
                parameters::version_specific_parameters::{
                    AuthorizationInfo, VersionSpecificParameter,
                },
                publish_namespace::PublishNamespace,
            };
            use bytes::BytesMut;
            #[test]
            fn with_parameter() {
                let bytes_array = [
                    0,  // Message Length(16)
                    19, // Message Length(16)
                    0,  // request id(u64)
                    2,  // Track Namespace(tuple): Number of elements
                    4,  // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    4,   // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    1,   // Number of Parameters (i)
                    2,   // Parameters (..): Parameter Type(AuthorizationInfo)
                    4,   // Parameters (..): Length
                    116, 101, 115, 116, // Parameters (..): Value("test")
                ];
                let mut buf = BytesMut::with_capacity(bytes_array.len());
                buf.extend_from_slice(&bytes_array);
                let depacketized_announce_message = PublishNamespace::decode(&mut buf).unwrap();
                let request_id = 0;
                let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
                let parameter_value = "test".to_string();
                let parameter = VersionSpecificParameter::AuthorizationInfo(
                    AuthorizationInfo::new(parameter_value.clone()),
                );
                let parameters = vec![parameter];
                let expected_announce_message =
                    PublishNamespace::new(request_id, track_namespace.clone(), parameters);

                assert_eq!(depacketized_announce_message, expected_announce_message);
            }

            #[test]
            fn without_parameter() {
                let bytes_array = [
                    0,  // Message Length(16)
                    13, // Message Length(16)
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
