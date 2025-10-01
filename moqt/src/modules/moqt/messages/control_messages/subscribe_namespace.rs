use crate::modules::moqt::messages::{
    control_messages::{
        util::{add_payload_length, validate_payload_length},
        version_specific_parameters::VersionSpecificParameter,
    },
    moqt_message::MOQTMessage,
    moqt_message_error::MOQTMessageError,
    moqt_payload::MOQTPayload,
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
use anyhow::{Context, Result};
use bytes::BytesMut;
use serde::Serialize;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SubscribeNamespace {
    pub(crate) request_id: u64,
    pub(crate) track_namespace_prefix: Vec<String>,
    number_of_parameters: u64,
    parameters: Vec<VersionSpecificParameter>,
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

    pub fn parameters(&self) -> &Vec<VersionSpecificParameter> {
        &self.parameters
    }
}

impl MOQTMessage for SubscribeNamespace {
    fn depacketize(buf: &mut BytesMut) -> Result<Self, MOQTMessageError> {
        if !validate_payload_length(buf) {
            return Err(MOQTMessageError::ProtocolViolation);
        }

        let request_id = match read_variable_integer_from_buffer(buf) {
            Ok(v) => v,
            Err(_) => return Err(MOQTMessageError::ProtocolViolation),
        };

        let track_namespace_prefix_tuple_length = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("track namespace prefix length")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let mut track_namespace_prefix_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_prefix_tuple_length {
            let track_namespace_prefix = String::from_utf8(
                read_variable_bytes_from_buffer(buf)
                    .map_err(|_| MOQTMessageError::ProtocolViolation)?,
            )
            .context("track namespace prefix")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
            track_namespace_prefix_tuple.push(track_namespace_prefix);
        }

        let number_of_parameters = read_variable_integer_from_buffer(buf)
            .context("number of parameters")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;

        let mut parameters = Vec::new();
        for _ in 0..number_of_parameters {
            let version_specific_parameter = VersionSpecificParameter::depacketize(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?;
            if let VersionSpecificParameter::Unknown(code) = version_specific_parameter {
                tracing::warn!("unknown track request parameter {}", code);
            } else {
                parameters.push(version_specific_parameter);
            }
        }

        Ok(SubscribeNamespace {
            request_id,
            track_namespace_prefix: track_namespace_prefix_tuple,
            number_of_parameters,
            parameters,
        })
    }

    fn packetize(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.extend(write_variable_integer(self.request_id));
        let track_namespace_prefix_tuple_length = self.track_namespace_prefix.len();
        payload.extend(write_variable_integer(
            track_namespace_prefix_tuple_length as u64,
        ));
        for track_namespace_prefix in &self.track_namespace_prefix {
            payload.extend(write_variable_bytes(
                &track_namespace_prefix.as_bytes().to_vec(),
            ));
        }
        payload.extend(write_variable_integer(self.parameters.len() as u64));
        for parameter in &self.parameters {
            parameter.packetize(&mut payload);
        }
        add_payload_length(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::messages::{
            control_messages::{
                subscribe_namespace::SubscribeNamespace,
                version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
            },
            moqt_message::MOQTMessage,
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
            let buf = subscribe_announces.packetize();

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
            let subscribe_announces = SubscribeNamespace::depacketize(&mut buf).unwrap();

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
