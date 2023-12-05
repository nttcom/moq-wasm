use anyhow::{Context, Result};
use serde::Serialize;

use crate::modules::variable_integer::{read_variable_integer_from_buffer, write_variable_integer};

use super::{moqt_payload::MOQTPayload, setup_parameters::SetupParameter};

#[derive(Debug, Serialize, Clone)]
pub struct ServerSetupMessage {
    pub selected_version: u32,
    pub number_of_parameters: u8,
    pub setup_parameters: Vec<SetupParameter>,
}

impl ServerSetupMessage {
    pub fn new(selected_version: u32, setup_parameters: Vec<SetupParameter>) -> Self {
        ServerSetupMessage {
            selected_version,
            number_of_parameters: setup_parameters.len() as u8,
            setup_parameters,
        }
    }
}

impl MOQTPayload for ServerSetupMessage {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let selected_version = u32::try_from(read_variable_integer_from_buffer(buf)?)
            .context("Depacketize elected version")?;

        let number_of_parameters = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("Depacketize number of parameters")?;

        let mut setup_parameters = vec![];
        for _ in 0..number_of_parameters {
            setup_parameters
                .push(SetupParameter::depacketize(buf).context("Depacketize setup parameter")?);
        }

        let server_setup_message = ServerSetupMessage {
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
}
