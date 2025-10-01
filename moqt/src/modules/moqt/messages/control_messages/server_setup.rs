use crate::modules::moqt::messages::{
    control_messages::{
        setup_parameters::SetupParameter,
        util::{add_payload_length, validate_payload_length},
    },
    moqt_message::MOQTMessage,
    moqt_message_error::MOQTMessageError,
    moqt_payload::MOQTPayload,
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
use anyhow::Context;
use bytes::BytesMut;
use serde::Serialize;

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

impl MOQTMessage for ServerSetup {
    fn depacketize(buf: &mut BytesMut) -> Result<Self, MOQTMessageError> {
        if !validate_payload_length(buf) {
            return Err(MOQTMessageError::ProtocolViolation);
        }

        let selected_version = u32::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("Depacketize selected version")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let number_of_parameters = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("Depacketize number of parameters")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let mut setup_parameters = vec![];
        for _ in 0..number_of_parameters {
            setup_parameters.push(
                SetupParameter::depacketize(buf)
                    .context("Depacketize setup parameter")
                    .map_err(|_| MOQTMessageError::ProtocolViolation)?,
            );
        }

        let server_setup_message = ServerSetup {
            selected_version,
            number_of_parameters,
            setup_parameters,
        };
        Ok(server_setup_message)
    }

    fn packetize(&self) -> BytesMut {
        let mut payload = BytesMut::new();

        let version_buf = write_variable_integer(self.selected_version as u64);
        payload.extend(version_buf);

        let number_of_parameters_buf = write_variable_integer(self.number_of_parameters as u64);
        payload.extend(number_of_parameters_buf);

        for setup_parameter in self.setup_parameters.iter() {
            setup_parameter.packetize(&mut payload);
        }
        add_payload_length(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::{
            constants::MOQ_TRANSPORT_VERSION,
            messages::{
                control_messages::{
                    server_setup::ServerSetup,
                    setup_parameters::{MaxSubscribeID, SetupParameter},
                },
                moqt_message::MOQTMessage,
            },
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let selected_version = MOQ_TRANSPORT_VERSION;
            let setup_parameters = vec![SetupParameter::MaxSubscribeID(MaxSubscribeID::new(2000))];
            let server_setup = ServerSetup::new(selected_version, setup_parameters.clone());
            let buf = server_setup.packetize();

            let expected_bytes_array = [
                13,  // Payload length
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
                13,  // Payload length
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
