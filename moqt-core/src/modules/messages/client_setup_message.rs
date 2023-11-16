use std::vec;

use anyhow::Result;

use crate::modules::variable_integer::{read_variable_integer_from_buffer, write_variable_integer};

use super::{moqt_payload::MOQTPayload, setup_parameters::SetupParameter};

pub(crate) struct ClientSetupMessage {
    pub(crate) number_of_supported_versions: u8,
    pub(crate) supported_versions: Vec<u32>,
    pub(crate) number_of_parameters: u8,
    pub(crate) setup_parameters: Vec<SetupParameter>,
}

impl MOQTPayload for ClientSetupMessage {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let number_of_supported_versions = u8::try_from(read_variable_integer_from_buffer(buf)?)?;

        let mut supported_versions = Vec::with_capacity(number_of_supported_versions as usize);
        for _ in 0..number_of_supported_versions {
            let supported_version = u32::try_from(read_variable_integer_from_buffer(buf)?)?;
            supported_versions.push(supported_version);
        }

        let number_of_parameters = u8::try_from(read_variable_integer_from_buffer(buf)?)?;

        let mut setup_parameters = vec![];
        for _ in 0..number_of_parameters {
            setup_parameters.push(SetupParameter::depacketize(buf)?);
        }

        let client_setup_message = ClientSetupMessage {
            number_of_supported_versions,
            supported_versions,
            number_of_parameters,
            setup_parameters,
        };

        Ok(client_setup_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        todo!()
    }
}

#[cfg(test)]
impl ClientSetupMessage {
    pub fn new(supported_versions: Vec<u32>, setup_parameters: Vec<SetupParameter>) -> Self {
        ClientSetupMessage {
            number_of_supported_versions: supported_versions.len() as u8,
            supported_versions,
            number_of_parameters: setup_parameters.len() as u8,
            setup_parameters,
        }
    }
}
