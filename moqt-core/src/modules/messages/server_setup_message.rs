use anyhow::Result;

use crate::modules::variable_integer::{read_variable_integer_from_buffer, write_variable_integer};

use super::{payload::Payload, setup_parameters::SetupParameter};

pub(crate) struct ServerSetupMessage {
    pub(crate) selected_version: u32,
    pub(crate) number_of_parameters: u8,
    pub(crate) setup_parameters: Vec<SetupParameter>,
}

impl ServerSetupMessage {
    pub(crate) fn new(selected_version: u32, setup_parameters: Vec<SetupParameter>) -> Self {
        ServerSetupMessage {
            selected_version,
            number_of_parameters: setup_parameters.len() as u8,
            setup_parameters,
        }
    }
}

impl Payload for ServerSetupMessage {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        todo!()
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
