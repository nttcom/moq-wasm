use crate::modules::moqt::messages::{
    control_message_type::ControlMessageType,
    control_messages::{
        util::{add_header, validate_header},
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
use std::any::Any;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct PublishNamespace {
    pub(crate) request_id: u64,
    pub(crate) track_namespace: Vec<String>,
    pub(crate) number_of_parameters: u8,
    pub(crate) parameters: Vec<VersionSpecificParameter>,
}

impl PublishNamespace {
    pub fn new(
        request_id: u64,
        track_namespace: Vec<String>,
        parameters: Vec<VersionSpecificParameter>,
    ) -> Self {
        let number_of_parameters = parameters.len() as u8;
        PublishNamespace {
            request_id,
            track_namespace,
            number_of_parameters,
            parameters,
        }
    }
    pub fn track_namespace(&self) -> Vec<String> {
        // TODO: consider `std::mem::take(&mut self.items)`
        self.track_namespace.clone()
    }
}

impl MOQTMessage for PublishNamespace {
    fn depacketize(buf: &mut BytesMut) -> Result<Self, MOQTMessageError> {
        validate_header(ControlMessageType::PublishNamespace as u8, buf)?;

        let request_id = match read_variable_integer_from_buffer(buf) {
            Ok(v) => v,
            Err(_) => return Err(MOQTMessageError::ProtocolViolation),
        };

        let track_namespace_tuple_length = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("track namespace length")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let mut track_namespace_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let track_namespace = String::from_utf8(
                read_variable_bytes_from_buffer(buf)
                    .map_err(|_| MOQTMessageError::ProtocolViolation)?,
            )
            .context("track namespace")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
            track_namespace_tuple.push(track_namespace);
        }
        let number_of_parameters = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("number of parameters")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let mut parameters = vec![];
        for _ in 0..number_of_parameters {
            let version_specific_parameter = VersionSpecificParameter::depacketize(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?;
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

        Ok(announce_message)
    }

    fn packetize(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.extend(write_variable_integer(self.request_id));
        let track_namespace_tuple_length = self.track_namespace.len();
        payload.extend(write_variable_integer(track_namespace_tuple_length as u64));
        for track_namespace in &self.track_namespace {
            payload.extend(write_variable_bytes(&track_namespace.as_bytes().to_vec()));
        }
        payload.extend(write_variable_integer(self.number_of_parameters as u64));
        // Parameters
        for param in &self.parameters {
            param.packetize(&mut payload);
        }

        tracing::trace!("Packetized Announce message.");
        add_header(ControlMessageType::PublishNamespace as u8, payload)
    }
    /// Method to enable downcasting from MOQTPayload to Announce
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    mod success {

        mod packetize {
            use crate::modules::moqt::messages::{
                control_messages::{
                    publish_namespace::PublishNamespace,
                    version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
                },
                moqt_message::MOQTMessage,
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
                let buf = announce_message.packetize();

                let expected_bytes_array = [
                    6,  // Message Type(u64)
                    19, // Message Length
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

                assert_eq!(buf.as_ref(), expected_bytes_array);
            }
            #[test]
            fn without_parameter() {
                let request_id = 0;
                let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
                let parameters = vec![];
                let announce_message =
                    PublishNamespace::new(request_id, track_namespace.clone(), parameters);
                let buf = announce_message.packetize();

                let expected_bytes_array = [
                    6,  // Message Type(u64)
                    13, // Message Length
                    0,  // request id(u64)
                    2,  // Track Namespace(tuple): Number of elements
                    4,  // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    4,   // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    0,   // Number of Parameters (i)
                ];

                assert_eq!(buf.as_ref(), expected_bytes_array);
            }
        }

        mod depacketize {
            use crate::modules::moqt::messages::{
                control_messages::{
                    publish_namespace::PublishNamespace,
                    version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
                },
                moqt_message::MOQTMessage,
            };
            use bytes::BytesMut;
            #[test]
            fn with_parameter() {
                let bytes_array = [
                    6,  // Message Type(u64)
                    19, // Message Length
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
                let depacketized_announce_message =
                    PublishNamespace::depacketize(&mut buf).unwrap();
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
                    6,  // Message Type(u64)
                    13, // Message Length
                    0,  // request id(u64)
                    2, // Track Namespace(tuple): Number of elements
                    4, // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    4,   // Track Namespace(b): Length
                    116, 101, 115, 116, // Track Namespace(b): Value("test")
                    0,   // Number of Parameters (i)
                ];
                let mut buf = BytesMut::with_capacity(bytes_array.len());
                buf.extend_from_slice(&bytes_array);
                let depacketized_announce_message =
                    PublishNamespace::depacketize(&mut buf).unwrap();

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
