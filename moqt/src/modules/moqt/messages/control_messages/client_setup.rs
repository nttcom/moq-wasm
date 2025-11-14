use crate::modules::{
    extensions::{bytes_reader::BytesReader, bytes_writer::BytesWriter},
    moqt::messages::{
        control_messages::{
            setup_parameters::SetupParameter,
            util::{add_payload_length, validate_payload_length},
        },
        moqt_message::MOQTMessage,
        moqt_message_error::MOQTMessageError,
        moqt_payload::MOQTPayload,
    },
};
use anyhow::Context;
use bytes::BytesMut;
use std::vec;

#[derive(Debug, Clone, PartialEq)]
pub struct ClientSetup {
    pub number_of_supported_versions: u64,
    pub supported_versions: Vec<u32>,
    pub(crate) number_of_parameters: u64,
    pub setup_parameters: Vec<SetupParameter>,
}

impl ClientSetup {
    pub fn new(supported_versions: Vec<u32>, setup_parameters: Vec<SetupParameter>) -> ClientSetup {
        ClientSetup {
            number_of_supported_versions: supported_versions.len() as u64,
            supported_versions,
            number_of_parameters: setup_parameters.len() as u64,
            setup_parameters,
        }
    }
}

impl MOQTMessage for ClientSetup {
    fn depacketize(buf: &mut BytesMut) -> Result<Self, MOQTMessageError> {
        if !validate_payload_length(buf) {
            return Err(MOQTMessageError::ProtocolViolation);
        }

        let number_of_supported_versions = buf
            .try_get_varint()
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;

        let mut supported_versions = Vec::with_capacity(number_of_supported_versions as usize);
        for _ in 0..number_of_supported_versions {
            let supported_version = buf
                .try_get_varint()
                .map_err(|_| MOQTMessageError::ProtocolViolation)?;
            supported_versions.push(supported_version as u32);
        }

        let number_of_parameters = buf
            .try_get_varint()
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
        payload.put_varint(self.number_of_supported_versions);
        for supported_version in &self.supported_versions {
            payload.put_varint(*supported_version as u64);
        }

        payload.put_varint(self.number_of_parameters);
        for setup_parameter in &self.setup_parameters {
            setup_parameter.packetize(&mut payload);
        }

        add_payload_length(payload)
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
                    setup_parameters::{MOQTimplementation, MaxSubscribeID, SetupParameter},
                },
                moqt_message::MOQTMessage,
            },
        };
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let supported_versions = vec![MOQ_TRANSPORT_VERSION];
            let moqt_implementation =
                SetupParameter::MOQTimplementation(MOQTimplementation::new("MOQ-WASM".to_string()));
            let setup_parameters = vec![
                SetupParameter::MaxSubscribeID(MaxSubscribeID::new(2000)),
                moqt_implementation,
            ];
            let client_setup = ClientSetup::new(supported_versions, setup_parameters.clone());
            let buf = client_setup.packetize();

            let expected_bytes_array = [
                0, 23,  // Payload length
                1,   // Number of Supported Versions (i)
                192, // Supported Version (i): Length(11 of 2MSB)
                0, 0, 0, 255, 0, 0, 14,  // Supported Version(i): Value(0xff000008) in 62bit
                2,   // Number of Parameters (i)
                2,   // Parameter Type (i): Type(MaxSubscribeID)
                71,  // Parameter Value (..): Length(01 of 2MSB)
                208, // Parameter Value (..): Value(2000) in 62bit
                7, 8, 77, 79, 81, 45, 87, 65, 83, 77,
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn depacketize() {
            let bytes_array = [
                0, 23,  // Payload length
                1,   // Number of Supported Versions (i)
                192, // Supported Version (i): Length(11 of 2MSB)
                0, 0, 0, 255, 0, 0, 14,  // Supported Version(i): Value(0xff000008) in 62bit
                2,   // Number of Parameters (i)
                2,   // Parameter Type (i): Type(MaxSubscribeID)
                71,  // Parameter Value (..): Length(01 of 2MSB)
                208, // Parameter Value (..): Value(2000) in 62bit
                7, 8, 77, 79, 81, 45, 87, 65, 83, 77,
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let depacketized_client_setup = ClientSetup::depacketize(&mut buf).unwrap();

            let supported_versions = vec![MOQ_TRANSPORT_VERSION];
            let moqt_implementaion =
                SetupParameter::MOQTimplementation(MOQTimplementation::new("MOQ-WASM".to_string()));
            let setup_parameters = vec![
                SetupParameter::MaxSubscribeID(MaxSubscribeID::new(2000)),
                moqt_implementaion,
            ];
            let expected_client_setup =
                ClientSetup::new(supported_versions, setup_parameters.clone());

            assert_eq!(depacketized_client_setup, expected_client_setup);
        }
    }
}
