use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::messages::parameters::setup_parameters::SetupParameter,
};
use bytes::BytesMut;

#[derive(Debug, Clone, PartialEq)]
pub struct ServerSetup {
    pub selected_version: u32,
    pub setup_parameters: SetupParameter,
}

impl ServerSetup {
    pub fn new(selected_version: u32, setup_parameters: SetupParameter) -> Self {
        ServerSetup {
            selected_version,
            setup_parameters,
        }
    }
}

impl ServerSetup {
    pub(crate) fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let selected_version = buf.try_get_varint().log_context("selected version").ok()?;
        let setup_parameters = SetupParameter::decode(buf)?;

        let server_setup_message = ServerSetup {
            selected_version: selected_version as u32,
            setup_parameters,
        };
        Some(server_setup_message)
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.selected_version as u64);
        payload.extend_from_slice(&self.setup_parameters.encode());
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::{
            control_plane::constants::MOQ_TRANSPORT_VERSION,
            control_plane::control_messages::messages::{
                parameters::setup_parameters::SetupParameter, server_setup::ServerSetup,
            },
        };
        use bytes::BytesMut;

        #[test]
        fn encode() {
            let selected_version = MOQ_TRANSPORT_VERSION;
            let setup_parameters = SetupParameter {
                max_request_id: 2000,
                path: None,
                authorization_token: vec![],
                max_auth_token_cache_size: None,
                authority: None,
                moq_implementation: Some("MOQ-WASM".to_string()),
            };
            let server_setup = ServerSetup::new(selected_version, setup_parameters.clone());
            let buf = server_setup.encode();

            let expected_bytes_array = [
                192, // Selected Version (i): Length(11 of 2MSB)
                0, 0, 0, 255, 0, 0, 14,  // Supported Version(i): Value(0xff000a) in 62bit
                2,   // Number of Parameters (i)
                2,   // Parameter Type (i): Type(MaxSubscribeID)
                71,  // Parameter Value (..): Length(01 of 2MSB)
                208, // Parameter Value (..): Value(2000) in 62bit
                7, 8, b'M', b'O', b'Q', b'-', b'W', b'A', b'S', b'M',
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn decode() {
            let bytes_array = [
                192, // Selected Version (i): Length(11 of 2MSB)
                0, 0, 0, 255, 0, 0, 14,  // Supported Version(i): Value(0xff00000a) in 62bit
                2,   // Number of Parameters (i)
                2,   // Parameter Type (i): Type(MaxSubscribeID)
                71,  // Parameter Value (..): Length(01 of 2MSB)
                208, // Parameter Value (..): Value(2000) in 62bit
                7, 8, b'M', b'O', b'Q', b'-', b'W', b'A', b'S', b'M',
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut cursor = std::io::Cursor::new(buf.as_ref());
            let depacketized_server_setup = ServerSetup::decode(&mut cursor).unwrap();
            assert_eq!(depacketized_server_setup.selected_version, 0xff00000e);
            assert_eq!(
                depacketized_server_setup.setup_parameters.max_request_id,
                2000
            );
            assert_eq!(
                depacketized_server_setup
                    .setup_parameters
                    .moq_implementation,
                Some("MOQ-WASM".to_string())
            );
        }
    }
}
