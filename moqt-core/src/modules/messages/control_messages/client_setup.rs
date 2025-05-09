use crate::{
    messages::{control_messages::setup_parameters::SetupParameter, moqt_payload::MOQTPayload},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
use anyhow::{Context, Result};
use bytes::BytesMut;
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
    fn depacketize(buf: &mut BytesMut) -> Result<Self> {
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

        Ok(client_setup_message)
    }

    fn packetize(&self, buf: &mut BytesMut) {
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
    /// Method to enable downcasting from MOQTPayload to ClientSetup
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod test {
    mod success {
        use crate::{
            constants::MOQ_TRANSPORT_VERSION,
            messages::{
                control_messages::{
                    client_setup::ClientSetup,
                    setup_parameters::{MaxSubscribeID, SetupParameter},
                },
                moqt_payload::MOQTPayload,
            },
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let supported_versions = vec![MOQ_TRANSPORT_VERSION];
            let setup_parameters = vec![SetupParameter::MaxSubscribeID(MaxSubscribeID::new(2000))];
            let client_setup = ClientSetup::new(supported_versions, setup_parameters.clone());
            let mut buf = BytesMut::new();
            client_setup.packetize(&mut buf);

            let expected_bytes_array = [
                1,   // Number of Supported Versions (i)
                192, // Supported Version (i): Length(11 of 2MSB)
                0, 0, 0, 255, 0, 0, 10,  // Supported Version(i): Value(0xff000008) in 62bit
                1,   // Number of Parameters (i)
                2,   // Parameter Type (i): Type(MaxSubscribeID)
                2,   // Parameter Length (i)
                71,  // Parameter Value (..): Length(01 of 2MSB)
                208, // Parameter Value (..): Value(2000) in 62bit
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn depacketize() {
            let bytes_array = [
                1,   // Number of Supported Versions (i)
                192, // Supported Version (i): Length(11 of 2MSB)
                0, 0, 0, 255, 0, 0, 10,  // Supported Version(i): Value(0xff000008) in 62bit
                1,   // Number of Parameters (i)
                2,   // Parameter Type (i): Type(MaxSubscribeID)
                2,   // Parameter Length (i)
                71,  // Parameter Value (..): Length(01 of 2MSB)
                208, // Parameter Value (..): Value(2000) in 62bit
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let depacketized_client_setup = ClientSetup::depacketize(&mut buf).unwrap();

            let supported_versions = vec![MOQ_TRANSPORT_VERSION];
            let setup_parameters = vec![SetupParameter::MaxSubscribeID(MaxSubscribeID::new(2000))];
            let expected_client_setup =
                ClientSetup::new(supported_versions, setup_parameters.clone());

            assert_eq!(depacketized_client_setup, expected_client_setup);
        }
    }
}
