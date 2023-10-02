use std::vec;

use anyhow::Result;

use crate::modules::variable_integer::{
    read_variable_integer, read_variable_integer_from_buffer, write_variable_integer,
};

use super::{
    payload::Payload,
    setup_parameters::{self, SetupParameter},
};

pub(crate) struct ClientSetupMessage {
    pub(crate) number_of_supported_versions: u8,
    pub(crate) supported_versions: Vec<u8>,
    pub(crate) setup_parameters: Vec<SetupParameter>,
}

impl Payload for ClientSetupMessage {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let number_of_supported_versions = u8::try_from(read_variable_integer_from_buffer(buf)?)?;

        let mut supported_versions = Vec::with_capacity(number_of_supported_versions as usize);
        for _ in 0..number_of_supported_versions {
            let supported_version = u8::try_from(read_variable_integer_from_buffer(buf)?)?;
            supported_versions.push(supported_version);
        }

        let mut setup_parameters = vec![];
        while !buf.is_empty() {
            setup_parameters.push(SetupParameter::depacketize(buf)?);
        }

        let client_setup_message = ClientSetupMessage {
            number_of_supported_versions,
            supported_versions,
            setup_parameters,
        };

        Ok(client_setup_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        todo!()
    }
}

pub(crate) struct ServerSetupMessage {
    pub(crate) selected_version: u8,
    pub(crate) setup_parameters: Vec<SetupParameter>,
}

impl ServerSetupMessage {
    pub(crate) fn new(selected_version: u8, setup_parameters: Vec<SetupParameter>) -> Self {
        ServerSetupMessage {
            selected_version,
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

        for setup_parameter in self.setup_parameters.iter() {
            setup_parameter.packetize(buf);
        }
    }
}
