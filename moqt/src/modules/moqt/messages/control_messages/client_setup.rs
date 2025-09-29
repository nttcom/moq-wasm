use crate::modules::moqt::messages::{
    control_message_type::ControlMessageType,
    control_messages::{
        setup_parameters::SetupParameter,
        util::{add_header, validate_header},
    },
    moqt_message::MOQTMessage,
    moqt_message_error::MOQTMessageError,
    moqt_payload::MOQTPayload,
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
use anyhow::Context;
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

impl MOQTMessage for ClientSetup {
    fn depacketize(buf: &mut BytesMut) -> Result<Self, MOQTMessageError> {
        validate_header(ControlMessageType::ClientSetup as u8, buf)?;

        let number_of_supported_versions = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("number of supported versions")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;

        let mut supported_versions = Vec::with_capacity(number_of_supported_versions as usize);
        for _ in 0..number_of_supported_versions {
            let supported_version = u32::try_from(
                read_variable_integer_from_buffer(buf)
                    .map_err(|_| MOQTMessageError::ProtocolViolation)?,
            )
            .context("supported version")
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
            supported_versions.push(supported_version);
        }

        let number_of_parameters = u8::try_from(
            read_variable_integer_from_buffer(buf)
                .map_err(|_| MOQTMessageError::ProtocolViolation)?,
        )
        .context("number of parameters")
        .map_err(|_| MOQTMessageError::ProtocolViolation)?;

        let mut setup_parameters = vec![];
        for _ in 0..number_of_parameters {
            setup_parameters.push(
                SetupParameter::depacketize(buf)
                    .map_err(|_| MOQTMessageError::ProtocolViolation)?,
            );
        }

        let client_setup_message = ClientSetup {
            number_of_supported_versions,
            supported_versions,
            number_of_parameters,
            setup_parameters,
        };

        Ok(client_setup_message)
    }

    fn packetize(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.extend(write_variable_integer(
            self.number_of_supported_versions as u64,
        ));
        for supported_version in &self.supported_versions {
            payload.extend(write_variable_integer(*supported_version as u64));
        }

        payload.extend(write_variable_integer(self.number_of_parameters as u64));
        for setup_parameter in &self.setup_parameters {
            setup_parameter.packetize(&mut payload);
        }

        add_header(ControlMessageType::ClientSetup as u8, payload)
    }
    /// Method to enable downcasting from MOQTPayload to ClientSetup
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod test {
    mod success {
        use crate::modules::moqt::{
            constants::MOQ_TRANSPORT_VERSION,
            messages::{
                control_messages::{
                    client_setup::ClientSetup,
                    setup_parameters::{MaxSubscribeID, SetupParameter},
                },
                moqt_message::MOQTMessage,
            },
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let supported_versions = vec![MOQ_TRANSPORT_VERSION];
            let setup_parameters = vec![SetupParameter::MaxSubscribeID(MaxSubscribeID::new(2000))];
            let client_setup = ClientSetup::new(supported_versions, setup_parameters.clone());
            let buf = client_setup.packetize();

            let expected_bytes_array = [
                32,  // Message Type
                14,  // Payload length
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
                32,  // Message Type
                14,  // Payload length
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
