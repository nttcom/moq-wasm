use crate::{
    modules::moqt::messages::variable_integer::{
        read_variable_integer_from_buffer, write_variable_integer,
    },
    modules::moqt::messages::{
        control_messages::setup_parameters::SetupParameter, moqt_payload::MOQTPayload,
    },
};
use anyhow::{Context, Result};
use bytes::BytesMut;
use serde::Serialize;
use std::any::Any;

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
    fn depacketize(buf: &mut BytesMut) -> Result<Self> {
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

    fn packetize(&self, buf: &mut BytesMut) {
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
mod tests {
    mod success {
        use crate::{
            modules::moqt::constants::MOQ_TRANSPORT_VERSION,
            modules::moqt::messages::{
                control_messages::{
                    server_setup::ServerSetup,
                    setup_parameters::{MaxSubscribeID, SetupParameter},
                },
                moqt_payload::MOQTPayload,
            },
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let selected_version = MOQ_TRANSPORT_VERSION;
            let setup_parameters = vec![SetupParameter::MaxSubscribeID(MaxSubscribeID::new(2000))];
            let server_setup = ServerSetup::new(selected_version, setup_parameters.clone());
            let mut buf = BytesMut::new();
            server_setup.packetize(&mut buf);

            let expected_bytes_array = [
                192, // Selected Version (i): Length(11 of 2MSB)
                0, 0, 0, 255, 0, 0, 10,  // Supported Version(i): Value(0xff000a) in 62bit
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
                192, // Selected Version (i): Length(11 of 2MSB)
                0, 0, 0, 255, 0, 0, 10,  // Supported Version(i): Value(0xff00000a) in 62bit
                1,   // Number of Parameters (i)
                2,   // Parameter Type (i): Type(MaxSubscribeID)
                2,   // Parameter Length (i)
                71,  // Parameter Value (..): Length(01 of 2MSB)
                208, // Parameter Value (..): Value(2000) in 62bit
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let depacketized_server_setup = ServerSetup::depacketize(&mut buf).unwrap();

            let selected_version = MOQ_TRANSPORT_VERSION;
            let setup_parameters = vec![SetupParameter::MaxSubscribeID(MaxSubscribeID::new(2000))];
            let expected_server_setup =
                ServerSetup::new(selected_version, setup_parameters.clone());

            assert_eq!(depacketized_server_setup, expected_server_setup);
        }
    }
}
