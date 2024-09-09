use anyhow::{Context, Result};
use serde::Serialize;
use std::any::Any;

use crate::modules::variable_integer::{read_variable_integer_from_buffer, write_variable_integer};

use super::{moqt_payload::MOQTPayload, setup_parameters::SetupParameter};

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ServerSetup {
    pub selected_version: u32,
    pub number_of_parameters: u8,
    pub setup_parameters: Vec<SetupParameter>,
}

impl ServerSetup {
    pub fn new(selected_version: u32, setup_parameters: Vec<SetupParameter>) -> Self {
        ServerSetup {
            selected_version,
            number_of_parameters: setup_parameters.len() as u8,
            setup_parameters,
        }
    }
}

impl MOQTPayload for ServerSetup {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let selected_version = u32::try_from(read_variable_integer_from_buffer(buf)?)
            .context("Depacketize selected version")?;

        let number_of_parameters = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("Depacketize number of parameters")?;

        let mut setup_parameters = vec![];
        for _ in 0..number_of_parameters {
            setup_parameters
                .push(SetupParameter::depacketize(buf).context("Depacketize setup parameter")?);
        }

        let server_setup_message = ServerSetup {
            selected_version,
            number_of_parameters,
            setup_parameters,
        };

        Ok(server_setup_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        let version_buf = write_variable_integer(self.selected_version as u64);
        buf.extend(version_buf);

        let number_of_parameters_buf = write_variable_integer(self.number_of_parameters as u64);
        buf.extend(number_of_parameters_buf);

        for setup_parameter in self.setup_parameters.iter() {
            setup_parameter.packetize(buf);
        }
    }
    /// Method to enable downcasting from MOQTPayload to ServerSetup
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::modules::variable_integer::write_variable_integer;
    use crate::{
        constants::MOQ_TRANSPORT_VERSION,
        messages::moqt_payload::MOQTPayload,
        modules::messages::{
            server_setup::ServerSetup,
            setup_parameters::{RoleCase, RoleParameter, SetupParameter},
        },
    };
    #[test]
    fn packetize_server_setup() {
        let selected_version = MOQ_TRANSPORT_VERSION;

        let role_parameter = RoleParameter::new(RoleCase::Both);
        let setup_parameters = vec![SetupParameter::RoleParameter(role_parameter.clone())];
        let setup_parameters_length = setup_parameters.len() as u8;

        let server_setup = ServerSetup::new(selected_version, setup_parameters.clone());
        let mut buf = bytes::BytesMut::new();
        server_setup.packetize(&mut buf);

        // Selected Version (i)
        let mut combined_bytes = Vec::from(write_variable_integer(selected_version as u64));
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
    fn depacketize_server_setup() {
        let selected_version = MOQ_TRANSPORT_VERSION;

        let role_parameter = RoleParameter::new(RoleCase::Both);
        let setup_parameters = vec![SetupParameter::RoleParameter(role_parameter.clone())];
        let setup_parameters_length = setup_parameters.len() as u8;

        let expected_server_setup = ServerSetup::new(selected_version, setup_parameters.clone());

        // Selected Version (i)
        let mut combined_bytes = Vec::from(write_variable_integer(selected_version as u64));
        // Number of Parameters (i)
        combined_bytes.extend(setup_parameters_length.to_be_bytes());
        // SETUP Parameters (..)
        combined_bytes.extend(vec![
            role_parameter.key as u8,
            role_parameter.value_length,
            role_parameter.value as u8,
        ]);

        let mut buf = bytes::BytesMut::from(combined_bytes.as_slice());
        let depacketized_server_setup = ServerSetup::depacketize(&mut buf).unwrap();

        assert_eq!(depacketized_server_setup, expected_server_setup);
    }
}
