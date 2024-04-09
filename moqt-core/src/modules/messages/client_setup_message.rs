use std::vec;

use anyhow::{Context, Result};

use crate::modules::variable_integer::{read_variable_integer_from_buffer, write_variable_integer};

use super::{moqt_payload::MOQTPayload, setup_parameters::SetupParameter};

#[derive(Debug, PartialEq)]
pub struct ClientSetupMessage {
    pub(crate) number_of_supported_versions: u8,
    pub(crate) supported_versions: Vec<u32>,
    pub(crate) number_of_parameters: u8,
    pub(crate) setup_parameters: Vec<SetupParameter>,
}

impl ClientSetupMessage {
    pub fn new(
        supported_versions: Vec<u32>,
        setup_parameters: Vec<SetupParameter>,
    ) -> ClientSetupMessage {
        ClientSetupMessage {
            number_of_supported_versions: supported_versions.len() as u8,
            supported_versions,
            number_of_parameters: setup_parameters.len() as u8,
            setup_parameters,
        }
    }
}

impl MOQTPayload for ClientSetupMessage {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let number_of_supported_versions = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("number of supported versions")?;
        tracing::debug!(
            "Depacketized client setup message number_of_supported_versions: {:#?}",
            number_of_supported_versions
        );

        let mut supported_versions = Vec::with_capacity(number_of_supported_versions as usize);
        for _ in 0..number_of_supported_versions {
            let supported_version = u32::try_from(read_variable_integer_from_buffer(buf)?)
                .context("supported version")?;
            supported_versions.push(supported_version);
        }
        tracing::debug!(
            "Depacketized client setup message supported_versions: {:#?}",
            supported_versions
        );

        let number_of_parameters = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("number of parameters")?;
        tracing::debug!(
            "Depacketized client setup message number_of_parameters: {:#?}",
            number_of_parameters
        );

        let mut setup_parameters = vec![];
        for _ in 0..number_of_parameters {
            setup_parameters.push(SetupParameter::depacketize(buf)?);
        }
        tracing::debug!(
            "Depacketized client setup message setup_parameters: {:#?}",
            setup_parameters
        );

        let client_setup_message = ClientSetupMessage {
            number_of_supported_versions,
            supported_versions,
            number_of_parameters,
            setup_parameters,
        };

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
    }
}

#[cfg(test)]
mod success {
    use crate::modules::variable_integer::write_variable_integer;
    use crate::{
        constants::MOQ_TRANSPORT_VERSION,
        messages::moqt_payload::MOQTPayload,
        modules::messages::{
            client_setup_message::ClientSetupMessage,
            setup_parameters::{RoleCase, RoleParameter, SetupParameter},
        },
    };
    #[test]
    fn packetize_client_setup() {
        let supported_versions = vec![MOQ_TRANSPORT_VERSION];
        let supported_versions_length = supported_versions.len() as u8;

        let role_parameter = RoleParameter::new(RoleCase::Injection);
        let setup_parameters = vec![SetupParameter::RoleParameter(role_parameter.clone())];
        let setup_parameters_length = setup_parameters.len() as u8;

        let client_setup = ClientSetupMessage::new(supported_versions, setup_parameters);
        let mut buf = bytes::BytesMut::new();
        client_setup.packetize(&mut buf);

        // Number of Supported Versions (i)
        let mut combined_bytes = Vec::from(supported_versions_length.to_be_bytes());
        // Supported Version (i)
        combined_bytes.extend(write_variable_integer(MOQ_TRANSPORT_VERSION as u64));
        // Number of Parameters (i)
        combined_bytes.extend(setup_parameters_length.to_be_bytes());
        // SETUP Parameters (..)
        combined_bytes.extend(vec![
            role_parameter.key as u8,
            role_parameter.value_length,
            role_parameter.value as u8,
        ]);

        assert_eq!(buf.as_ref(), combined_bytes.as_slice());
    }

    #[test]
    fn depacketize_client_setup() {
        let supported_versions = vec![MOQ_TRANSPORT_VERSION];
        let supported_versions_length = supported_versions.len() as u8;

        let role_parameter = RoleParameter::new(RoleCase::Injection);
        let setup_parameters = vec![SetupParameter::RoleParameter(role_parameter.clone())];
        let setup_parameters_length = setup_parameters.len() as u8;

        let expected_client_setup = ClientSetupMessage::new(supported_versions, setup_parameters);

        // Number of Supported Versions (i)
        let mut combined_bytes = Vec::from(supported_versions_length.to_be_bytes());
        // Supported Version (i)
        combined_bytes.extend(write_variable_integer(MOQ_TRANSPORT_VERSION as u64));
        // Number of Parameters (i)
        combined_bytes.extend(setup_parameters_length.to_be_bytes());
        // SETUP Parameters (..)
        combined_bytes.extend(vec![
            role_parameter.key as u8,
            role_parameter.value_length,
            role_parameter.value as u8,
        ]);

        let mut buf = bytes::BytesMut::from(combined_bytes.as_slice());
        let depacketized_client_setup = ClientSetupMessage::depacketize(&mut buf).unwrap();

        assert_eq!(depacketized_client_setup, expected_client_setup);
    }

    // TODO: Add tests for PathParameter when implementing QUIC Version
}
