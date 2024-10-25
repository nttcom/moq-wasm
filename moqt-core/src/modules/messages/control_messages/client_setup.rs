use super::setup_parameters::SetupParameter;
use crate::messages::moqt_payload::MOQTPayload;
use crate::modules::variable_integer::{read_variable_integer_from_buffer, write_variable_integer};
use anyhow::{Context, Result};
use std::{any::Any, vec};

#[derive(Debug, Clone, PartialEq)]
pub struct ClientSetup {
    pub number_of_supported_versions: u8,
    pub supported_versions: Vec<u32>,
    pub(crate) number_of_parameters: u8,
    pub setup_parameters: Vec<SetupParameter>,
}

impl ClientSetup {
    pub fn new(supported_versions: Vec<u32>, setup_parameters: Vec<SetupParameter>) -> ClientSetup {
        ClientSetup {
            number_of_supported_versions: supported_versions.len() as u8,
            supported_versions,
            number_of_parameters: setup_parameters.len() as u8,
            setup_parameters,
        }
    }
}

impl MOQTPayload for ClientSetup {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let number_of_supported_versions = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("number of supported versions")?;

        let mut supported_versions = Vec::with_capacity(number_of_supported_versions as usize);
        for _ in 0..number_of_supported_versions {
            let supported_version = u32::try_from(read_variable_integer_from_buffer(buf)?)
                .context("supported version")?;
            supported_versions.push(supported_version);
        }

        let number_of_parameters = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("number of parameters")?;

        let mut setup_parameters = vec![];
        for _ in 0..number_of_parameters {
            setup_parameters.push(SetupParameter::depacketize(buf)?);
        }

        let client_setup_message = ClientSetup {
            number_of_supported_versions,
            supported_versions,
            number_of_parameters,
            setup_parameters,
        };

        tracing::trace!("Depacketized Client Setup message.");

        Ok(client_setup_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        /*
            Client SETUP Message Payload {
                Number of Supported Versions (i),
                Supported Version (i) ...,
                Number of Parameters (i) ...,
                SETUP Parameters (..) ...,
            }
        */
        buf.extend(write_variable_integer(
            self.number_of_supported_versions as u64,
        ));
        for supported_version in &self.supported_versions {
            buf.extend(write_variable_integer(*supported_version as u64));
        }

        buf.extend(write_variable_integer(self.number_of_parameters as u64));
        for setup_parameter in &self.setup_parameters {
            setup_parameter.packetize(buf);
        }

        tracing::trace!("Packetized Client Setup message.");
    }
    /// Method to enable downcasting from MOQTPayload to ClientSetup
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::{
        constants::MOQ_TRANSPORT_VERSION,
        messages::moqt_payload::MOQTPayload,
        modules::messages::control_messages::{
            client_setup::ClientSetup,
            setup_parameters::{Role, RoleCase, SetupParameter},
        },
    };
    use bytes::BytesMut;
    #[test]
    fn packetize_client_setup() {
        let supported_versions = vec![MOQ_TRANSPORT_VERSION];
        let role_parameter = Role::new(RoleCase::Subscriber);
        let setup_parameters = vec![SetupParameter::Role(role_parameter.clone())];
        let client_setup = ClientSetup::new(supported_versions, setup_parameters.clone());
        let mut buf = bytes::BytesMut::new();
        client_setup.packetize(&mut buf);

        let expected_bytes_array = [
            1,   // Number of Supported Versions (i)
            192, // Supported Version (i): Length(11 of 2MSB)
            0, 0, 0, 255, 0, 0, 6, // Supported Version(i): Value(0xff000006) in 62bit
            1, // Number of Parameters (i)
            0, // SETUP Parameters (..): Type(Role)
            1, // SETUP Parameters (..): Length
            2, // SETUP Parameters (..): Role(Subscriber)
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn depacketize_client_setup() {
        let bytes_array = [
            1,   // Number of Supported Versions (i)
            192, // Supported Version (i): Length(11 of 2MSB)
            0, 0, 0, 255, 0, 0, 6, // Supported Version(i): Value(0xff000006) in 62bit
            1, // Number of Parameters (i)
            0, // SETUP Parameters (..): Type(Role)
            1, // SETUP Parameters (..): Length
            2, // SETUP Parameters (..): Role(Subscriber)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_client_setup = ClientSetup::depacketize(&mut buf).unwrap();

        let supported_versions = vec![MOQ_TRANSPORT_VERSION];
        let role_parameter = Role::new(RoleCase::Subscriber);
        let setup_parameters = vec![SetupParameter::Role(role_parameter.clone())];
        let expected_client_setup = ClientSetup::new(supported_versions, setup_parameters.clone());

        assert_eq!(depacketized_client_setup, expected_client_setup);
    }
}
