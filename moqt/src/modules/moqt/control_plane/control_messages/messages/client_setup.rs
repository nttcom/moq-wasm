use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::messages::parameters::setup_parameters::SetupParameter,
};
use bytes::BytesMut;

#[derive(Debug, Clone, PartialEq)]
pub struct ClientSetup {
    pub number_of_supported_versions: u64,
    pub supported_versions: Vec<u32>,
    pub setup_parameters: SetupParameter,
}

impl ClientSetup {
    pub fn new(supported_versions: Vec<u32>, setup_parameters: SetupParameter) -> ClientSetup {
        ClientSetup {
            number_of_supported_versions: supported_versions.len() as u64,
            supported_versions,
            setup_parameters,
        }
    }
}

impl ClientSetup {
    pub(crate) fn decode(buf: &mut BytesMut) -> Option<Self> {
        let number_of_supported_versions = buf
            .try_get_varint()
            .log_context("number_of_supported_versions")
            .ok()?;

        let mut supported_versions = Vec::with_capacity(number_of_supported_versions as usize);
        for _ in 0..number_of_supported_versions {
            let supported_version = buf.try_get_varint().log_context("supported_version").ok()?;
            supported_versions.push(supported_version as u32);
        }
        let setup_parameters = SetupParameter::decode(buf)?;

        let client_setup_message = ClientSetup {
            number_of_supported_versions,
            supported_versions,
            setup_parameters,
        };

        Some(client_setup_message)
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.number_of_supported_versions);
        for supported_version in &self.supported_versions {
            payload.put_varint(*supported_version as u64);
        }
        payload.unsplit(self.setup_parameters.encode());
        payload
    }
}

#[cfg(test)]
mod test {
    mod success {
        use crate::modules::moqt::control_plane::{
            constants::MOQ_TRANSPORT_VERSION,
            control_messages::messages::{
                client_setup::ClientSetup, parameters::setup_parameters::SetupParameter,
            },
        };
        use bytes::BytesMut;

        #[test]
        fn encode() {
            let supported_versions = vec![MOQ_TRANSPORT_VERSION];
            let setup_params = SetupParameter {
                max_request_id: 2000,
                path: None,
                authorization_token: vec![],
                max_auth_token_cache_size: None,
                authority: None,
                moq_implementation: Some("MOQ-WASM".to_string()),
            };
            let client_setup = ClientSetup::new(supported_versions, setup_params);
            let buf = client_setup.encode();

            let expected_bytes_array = [
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
        fn decode() {
            let bytes_array = [
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
            let depacketized_client_setup = ClientSetup::decode(&mut buf).unwrap();

            assert_eq!(depacketized_client_setup.number_of_supported_versions, 1);
            assert_eq!(
                depacketized_client_setup.supported_versions,
                vec![0xff00000e]
            );
            assert_eq!(
                depacketized_client_setup.setup_parameters.max_request_id,
                2000
            );
            assert_eq!(
                depacketized_client_setup
                    .setup_parameters
                    .moq_implementation,
                Some("MOQ-WASM".to_string())
            );
        }
    }
}
