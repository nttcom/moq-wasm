use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::messages::{
        control_messages::{
            util::{add_payload_length, validate_payload_length},
            version_specific_parameters::VersionSpecificParameter,
        },
        moqt_payload::MOQTPayload,
    },
};
use bytes::BytesMut;
use serde::Serialize;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SubscribeNamespace {
    pub(crate) request_id: u64,
    pub(crate) track_namespace_prefix: Vec<String>,
    number_of_parameters: u64,
    pub(crate) parameters: Vec<VersionSpecificParameter>,
}

impl SubscribeNamespace {
    pub fn new(
        request_id: u64,
        track_namespace_prefix: Vec<String>,
        parameters: Vec<VersionSpecificParameter>,
    ) -> Self {
        let number_of_parameters = parameters.len() as u64;
        SubscribeNamespace {
            request_id,
            track_namespace_prefix,
            number_of_parameters,
            parameters,
        }
    }
}

impl SubscribeNamespace {
    pub(crate) fn decode(buf: &mut BytesMut) -> Option<Self> {
        if !validate_payload_length(buf) {
            return None;
        }

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

        let mut parameters = Vec::new();
        for _ in 0..number_of_parameters {
            let version_specific_parameter = VersionSpecificParameter::depacketize(buf).ok()?;
            if let VersionSpecificParameter::Unknown(code) = version_specific_parameter {
                tracing::warn!("unknown track request parameter {}", code);
            } else {
                parameters.push(version_specific_parameter);
            }
        }

        Some(SubscribeNamespace {
            request_id,
            track_namespace_prefix: track_namespace_prefix_tuple,
            number_of_parameters,
            parameters,
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
        payload.put_varint(self.number_of_parameters);
        for parameter in &self.parameters {
            parameter.packetize(&mut payload);
        }
        add_payload_length(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::control_plane::messages::control_messages::{
            subscribe_namespace::SubscribeNamespace,
            version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let request_id = 0;
            let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let parameters = vec![version_specific_parameter];
            let subscribe_announces =
                SubscribeNamespace::new(request_id, track_namespace_prefix.clone(), parameters);
            let buf = subscribe_announces.encode();

            let expected_bytes_array = [
                19, // Message Length(i)
                0,  // Request ID(i)
                2,  // Track Namespace Prefix(tuple): Number of elements
                4,  // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                4,   // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                1,   // Parameters (..): Number of Parameters
                2,   // Parameter Type (i): AuthorizationInfo
                4,   // Parameter Length
                116, 101, 115, 116, // Parameter Value (..): test
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize() {
            let bytes_array = [
                19, // Message Length(i)
                0,  // Request ID(i)
                2,  // Track Namespace Prefix(tuple): Number of elements
                4,  // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                4,   // Track Namespace Prefix(b): Length
                116, 101, 115, 116, // Track Namespace Prefix(b): Value("test")
                1,   // Parameters (..): Number of Parameters
                2,   // Parameter Type (i): AuthorizationInfo
                4,   // Parameter Length
                116, 101, 115, 116, // Parameter Value (..): test
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let subscribe_announces = SubscribeNamespace::decode(&mut buf).unwrap();

            let request_id = 0;
            let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let parameters = vec![version_specific_parameter];
            let expected_subscribe_announces =
                SubscribeNamespace::new(request_id, track_namespace_prefix, parameters);

            assert_eq!(subscribe_announces, expected_subscribe_announces);
        }
    }
}
