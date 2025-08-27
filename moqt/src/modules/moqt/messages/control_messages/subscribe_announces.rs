use crate::{
    modules::moqt::messages::variable_bytes::{
        read_variable_bytes_from_buffer, write_variable_bytes,
    },
    modules::moqt::messages::variable_integer::{
        read_variable_integer_from_buffer, write_variable_integer,
    },
    modules::moqt::messages::{
        control_messages::version_specific_parameters::VersionSpecificParameter,
        moqt_payload::MOQTPayload,
    },
};
use anyhow::{Context, Result};
use bytes::BytesMut;
use serde::Serialize;
use std::any::Any;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SubscribeAnnounces {
    track_namespace_prefix: Vec<String>,
    number_of_parameters: u64,
    parameters: Vec<VersionSpecificParameter>,
}

impl SubscribeAnnounces {
    pub fn new(
        track_namespace_prefix: Vec<String>,
        parameters: Vec<VersionSpecificParameter>,
    ) -> Self {
        let number_of_parameters = parameters.len() as u64;
        SubscribeAnnounces {
            track_namespace_prefix,
            number_of_parameters,
            parameters,
        }
    }

    pub fn track_namespace_prefix(&self) -> &Vec<String> {
        &self.track_namespace_prefix
    }

    pub fn parameters(&self) -> &Vec<VersionSpecificParameter> {
        &self.parameters
    }
}

impl MOQTPayload for SubscribeAnnounces {
    fn depacketize(buf: &mut BytesMut) -> Result<Self> {
        let track_namespace_prefix_tuple_length =
            u8::try_from(read_variable_integer_from_buffer(buf)?)
                .context("track namespace prefix length")?;
        let mut track_namespace_prefix_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_prefix_tuple_length {
            let track_namespace_prefix = String::from_utf8(read_variable_bytes_from_buffer(buf)?)
                .context("track namespace prefix")?;
            track_namespace_prefix_tuple.push(track_namespace_prefix);
        }

        let number_of_parameters =
            read_variable_integer_from_buffer(buf).context("number of parameters")?;

        let mut parameters = Vec::new();
        for _ in 0..number_of_parameters {
            let version_specific_parameter = VersionSpecificParameter::depacketize(buf)?;
            if let VersionSpecificParameter::Unknown(code) = version_specific_parameter {
                tracing::warn!("unknown track request parameter {}", code);
            } else {
                parameters.push(version_specific_parameter);
            }
        }

        Ok(SubscribeAnnounces {
            track_namespace_prefix: track_namespace_prefix_tuple,
            number_of_parameters,
            parameters,
        })
    }

    fn packetize(&self, buf: &mut BytesMut) {
        let track_namespace_prefix_tuple_length = self.track_namespace_prefix.len();
        buf.extend(write_variable_integer(
            track_namespace_prefix_tuple_length as u64,
        ));
        for track_namespace_prefix in &self.track_namespace_prefix {
            buf.extend(write_variable_bytes(
                &track_namespace_prefix.as_bytes().to_vec(),
            ));
        }
        buf.extend(write_variable_integer(self.parameters.len() as u64));
        for parameter in &self.parameters {
            parameter.packetize(buf);
        }
    }
    /// Method to enable downcasting from MOQTPayload to SubscribeAnnounces
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::messages::{
            control_messages::{
                subscribe_announces::SubscribeAnnounces,
                version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
            },
            moqt_payload::MOQTPayload,
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let parameters = vec![version_specific_parameter];
            let subscribe_announces =
                SubscribeAnnounces::new(track_namespace_prefix.clone(), parameters);
            let mut buf = BytesMut::new();
            subscribe_announces.packetize(&mut buf);

            let expected_bytes_array = [
                2, // Track Namespace Prefix(tuple): Number of elements
                4, // Track Namespace Prefix(b): Length
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
                2, // Track Namespace Prefix(tuple): Number of elements
                4, // Track Namespace Prefix(b): Length
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
            let subscribe_announces = SubscribeAnnounces::depacketize(&mut buf).unwrap();

            let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let parameters = vec![version_specific_parameter];
            let expected_subscribe_announces =
                SubscribeAnnounces::new(track_namespace_prefix, parameters);

            assert_eq!(subscribe_announces, expected_subscribe_announces);
        }
    }
}
