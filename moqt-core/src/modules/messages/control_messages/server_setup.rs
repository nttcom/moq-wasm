use anyhow::{Context, Result};
use serde::Serialize;
use std::any::Any;

use crate::modules::variable_integer::{read_variable_integer_from_buffer, write_variable_integer};

use super::setup_parameters::SetupParameter;
use crate::messages::moqt_payload::MOQTPayload;

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

        tracing::trace!("Depacketized Server Setup message.");

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

        tracing::trace!("Packetized Server Setup message.");
    }
    /// Method to enable downcasting from MOQTPayload to ServerSetup
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
            server_setup::ServerSetup,
            setup_parameters::{Role, RoleCase, SetupParameter},
        },
    };
    use bytes::BytesMut;
    #[test]
    fn packetize_server_setup() {
        let selected_version = MOQ_TRANSPORT_VERSION;
        let role_parameter = Role::new(RoleCase::PubSub);
        let setup_parameters = vec![SetupParameter::Role(role_parameter.clone())];
        let server_setup = ServerSetup::new(selected_version, setup_parameters.clone());
        let mut buf = bytes::BytesMut::new();
        server_setup.packetize(&mut buf);

        let expected_bytes_array = [
            192, // Selected Version (i): Length(11 of 2MSB)
            0, 0, 0, 255, 0, 0, 1, // Supported Version(i): Value(0xff000001) in 62bit
            1, // Number of Parameters (i)
            0, // SETUP Parameters (..): Type(Role)
            1, // SETUP Parameters (..): Length
            3, // SETUP Parameters (..): Value(PubSub)
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn depacketize_server_setup() {
        let bytes_array = [
            192, // Selected Version (i): Length(11 of 2MSB)
            0, 0, 0, 255, 0, 0, 1, // Supported Version(i): Value(0xff000001) in 62bit
            1, // Number of Parameters (i)
            0, // SETUP Parameters (..): Type(Role)
            1, // SETUP Parameters (..): Length
            3, // SETUP Parameters (..): Value(PubSub)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_server_setup = ServerSetup::depacketize(&mut buf).unwrap();

        let selected_version = MOQ_TRANSPORT_VERSION;
        let role_parameter = Role::new(RoleCase::PubSub);
        let setup_parameters = vec![SetupParameter::Role(role_parameter.clone())];
        let expected_server_setup = ServerSetup::new(selected_version, setup_parameters.clone());

        assert_eq!(depacketized_server_setup, expected_server_setup);
    }
}
