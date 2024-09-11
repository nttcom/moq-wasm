use std::{any::Any, io::Cursor};

use anyhow::{Context, Result};

use crate::{
    modules::{
        messages::version_specific_parameters::VersionSpecificParameter,
        variable_integer::read_variable_integer_from_buffer,
    },
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::write_variable_integer,
};

use super::moqt_payload::MOQTPayload;

#[derive(Debug, Clone, PartialEq)]
pub struct Announce {
    pub(crate) track_namespace: String,
    pub(crate) number_of_parameters: u8,
    pub(crate) parameters: Vec<VersionSpecificParameter>,
}

impl Announce {
    pub fn new(
        track_namespace: String,
        number_of_parameters: u8,
        parameters: Vec<VersionSpecificParameter>,
    ) -> Self {
        Announce {
            track_namespace,
            number_of_parameters,
            parameters,
        }
    }
    pub fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
}

impl MOQTPayload for Announce {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let read_cur = Cursor::new(&buf[..]);
        tracing::debug!("read_cur! {:?}", read_cur);
        let track_namespace =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("track namespace")?;
        let number_of_parameters = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("number of parameters")?;
        let mut parameters = vec![];
        for _ in 0..number_of_parameters {
            let param = VersionSpecificParameter::depacketize(buf)?;
            parameters.push(param);
        }

        let announce_message = Announce {
            track_namespace,
            number_of_parameters,
            parameters,
        };

        tracing::trace!("Depacketized Announce message.");

        Ok(announce_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        /*
            ANNOUNCE Message {
                Track Namespace(b),
                Number of Parameters (i),
                Parameters (..) ...,
            }
        */

        // Track Namespace
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));
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
    use crate::messages::moqt_payload::MOQTPayload;
    use crate::messages::version_specific_parameters::AuthorizationInfo;
    use crate::modules::messages::announce::Announce;
    use crate::modules::messages::version_specific_parameters::VersionSpecificParameter;
    use bytes::BytesMut;

    #[test]
    fn packetize_announce_with_parameter() {
        let track_namespace = "live.example.com".to_string();
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
            16, // track_namespace length
            108, 105, 118, 101, 46, 101, 120, 97, 109, 112, 108, 101, 46, 99, 111,
            109, // track_namespace bytes("live.example.com")
            1,   // number_of_parameters
            2,   // parameter type
            4,   // parameter length
            116, 101, 115, 116, // parameter bytes("test")
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn depacketize_announce_with_parameter() {
        let bytes_array = [
            16, // track_namespace length
            108, 105, 118, 101, 46, 101, 120, 97, 109, 112, 108, 101, 46, 99, 111,
            109, // track_namespace bytes("live.example.com")
            1,   // number_of_parameters
            2,   // parameter type
            4,   // parameter length
            116, 101, 115, 116, // parameter bytes("test")
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_announce_message = Announce::depacketize(&mut buf).unwrap();

        let track_namespace = "live.example.com".to_string();
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
        let track_namespace = "live.example.com".to_string();
        let number_of_parameters = 0;
        let parameters = vec![];
        let announce_message =
            Announce::new(track_namespace.clone(), number_of_parameters, parameters);
        let mut buf = bytes::BytesMut::new();
        announce_message.packetize(&mut buf);

        let expected_bytes_array = [
            16, // track_namespace length
            108, 105, 118, 101, 46, 101, 120, 97, 109, 112, 108, 101, 46, 99, 111,
            109, // track_namespace bytes("live.example.com")
            0,   // number_of_parameters
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn depacketize_announce_without_parameter() {
        let bytes_array = [
            16, // track_namespace length
            108, 105, 118, 101, 46, 101, 120, 97, 109, 112, 108, 101, 46, 99, 111,
            109, // track_namespace bytes("live.example.com")
            0,   // number_of_parameters
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_announce_message = Announce::depacketize(&mut buf).unwrap();

        let track_namespace = "live.example.com".to_string();
        let number_of_parameters = 0;
        let parameters = vec![];
        let expected_announce_message =
            Announce::new(track_namespace.clone(), number_of_parameters, parameters);

        assert_eq!(depacketized_announce_message, expected_announce_message);
    }
}
