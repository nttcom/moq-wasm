use std::{any::Any, io::Cursor};

use anyhow::{Context, Result};

use crate::{
    modules::{
        messages::control_messages::version_specific_parameters::VersionSpecificParameter,
        variable_integer::read_variable_integer_from_buffer,
    },
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::write_variable_integer,
};

use crate::messages::moqt_payload::MOQTPayload;

#[derive(Debug, Clone, PartialEq)]
pub struct Announce {
    pub(crate) track_namespace: Vec<String>,
    pub(crate) number_of_parameters: u8,
    pub(crate) parameters: Vec<VersionSpecificParameter>,
}

impl Announce {
    pub fn new(
        track_namespace: Vec<String>,
        number_of_parameters: u8,
        parameters: Vec<VersionSpecificParameter>,
    ) -> Self {
        Announce {
            track_namespace,
            number_of_parameters,
            parameters,
        }
    }
    pub fn track_namespace(&self) -> &Vec<String> {
        &self.track_namespace
    }
}

impl MOQTPayload for Announce {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let read_cur = Cursor::new(&buf[..]);
        tracing::debug!("read_cur! {:?}", read_cur);
        let track_namespace_tuple_length = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("track namespace length")?;
        let mut track_namespace_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let track_namespace = String::from_utf8(read_variable_bytes_from_buffer(buf)?)
                .context("track namespace")?;
            track_namespace_tuple.push(track_namespace);
        }
        let number_of_parameters = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("number of parameters")?;
        let mut parameters = vec![];
        for _ in 0..number_of_parameters {
            let version_specific_parameter = VersionSpecificParameter::depacketize(buf)?;
            if let VersionSpecificParameter::Unknown(code) = version_specific_parameter {
                tracing::warn!("unknown track request parameter {}", code);
            } else {
                parameters.push(version_specific_parameter);
            }
        }

        let announce_message = Announce {
            track_namespace: track_namespace_tuple,
            number_of_parameters,
            parameters,
        };

        tracing::trace!("Depacketized Announce message.");

        Ok(announce_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        // Track Namespace Number of elements
        let track_namespace_tuple_length = self.track_namespace.len();
        buf.extend(write_variable_integer(track_namespace_tuple_length as u64));
        for track_namespace in &self.track_namespace {
            // Track Namespace
            buf.extend(write_variable_bytes(&track_namespace.as_bytes().to_vec()));
        }
        // Number of Parameters
        buf.extend(write_variable_integer(self.number_of_parameters as u64));
        // Parameters
        for param in &self.parameters {
            param.packetize(buf);
        }

        tracing::trace!("Packetized Announce message.");
    }
    /// Method to enable downcasting from MOQTPayload to Announce
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::messages::control_messages::version_specific_parameters::AuthorizationInfo;
    use crate::messages::moqt_payload::MOQTPayload;
    use crate::modules::messages::control_messages::announce::Announce;
    use crate::modules::messages::control_messages::version_specific_parameters::VersionSpecificParameter;
    use bytes::BytesMut;

    #[test]
    fn packetize_announce_with_parameter() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let number_of_parameters = 1;

        let parameter_value = "test".to_string();
        let parameter = VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(
            parameter_value.clone(),
        ));
        let parameters = vec![parameter];
        let announce_message =
            Announce::new(track_namespace.clone(), number_of_parameters, parameters);
        let mut buf = bytes::BytesMut::new();
        announce_message.packetize(&mut buf);

        let expected_bytes_array = [
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
    fn depacketize_announce_with_parameter() {
        let bytes_array = [
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
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_announce_message = Announce::depacketize(&mut buf).unwrap();

        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let number_of_parameters = 1;
        let parameter_value = "test".to_string();
        let parameter = VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(
            parameter_value.clone(),
        ));
        let parameters = vec![parameter];
        let expected_announce_message =
            Announce::new(track_namespace.clone(), number_of_parameters, parameters);

        assert_eq!(depacketized_announce_message, expected_announce_message);
    }

    #[test]
    fn packetize_announce_without_parameter() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let number_of_parameters = 0;
        let parameters = vec![];
        let announce_message =
            Announce::new(track_namespace.clone(), number_of_parameters, parameters);
        let mut buf = bytes::BytesMut::new();
        announce_message.packetize(&mut buf);

        let expected_bytes_array = [
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
    fn depacketize_announce_without_parameter() {
        let bytes_array = [
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            0,   // Number of Parameters (i)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_announce_message = Announce::depacketize(&mut buf).unwrap();

        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let number_of_parameters = 0;
        let parameters = vec![];
        let expected_announce_message =
            Announce::new(track_namespace.clone(), number_of_parameters, parameters);

        assert_eq!(depacketized_announce_message, expected_announce_message);
    }
}
