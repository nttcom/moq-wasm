use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt},
    moqt::control_plane::control_messages::messages::parameters::setup_parameters::SetupParameter,
};
use anyhow::Context;
use bytes::{Buf, BytesMut};

#[derive(Debug, Clone, PartialEq)]
pub struct ClientSetup {
    pub supported_versions: Vec<u32>,
    pub setup_parameters: SetupParameter,
}

impl ClientSetup {
    pub fn new(supported_versions: Vec<u32>, setup_parameters: SetupParameter) -> ClientSetup {
        ClientSetup {
            supported_versions,
            setup_parameters,
        }
    }
}

impl ClientSetup {
    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<Self> {
        let number_of_supported_versions = buf
            .try_get_varint()
            .context("number_of_supported_versions")?;

        // Each encoded version takes at least one byte, so a count above the
        // remaining payload cannot be satisfied.
        if number_of_supported_versions > buf.remaining() as u64 {
            anyhow::bail!(
                "number_of_supported_versions {} > remaining {}",
                number_of_supported_versions,
                buf.remaining()
            );
        }

        let mut supported_versions = Vec::new();
        for _ in 0..number_of_supported_versions {
            let supported_version = buf.try_get_varint().context("supported_version")?;
            supported_versions.push(u32::try_from(supported_version).with_context(|| {
                format!("supported version {supported_version:#x} exceeds 32 bits")
            })?);
        }
        let setup_parameters = SetupParameter::decode(buf)?;

        let client_setup_message = ClientSetup {
            supported_versions,
            setup_parameters,
        };

        Ok(client_setup_message)
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.supported_versions.len() as u64);
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
                trace_context: false,
                traceparent: None,
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
            let mut buf = std::io::Cursor::new(&buf[..]);
            let depacketized_client_setup = ClientSetup::decode(&mut buf).unwrap();

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

    mod failure {
        use crate::modules::moqt::control_plane::control_messages::messages::client_setup::ClientSetup;

        #[test]
        fn decode_version_above_u32_is_rejected() {
            // Arrange: the version's low 32 bits are draft-14
            let bytes_array = [
                1,   // Number of Supported Versions (i)
                192, // Supported Version (i): Length(11 of 2MSB)
                0, 0, 1, 255, 0, 0, 14, // Supported Version (i): Value(0x1_ff00000e) in 62bit
                0,  // Number of Parameters (i)
            ];
            let mut buf = std::io::Cursor::new(&bytes_array[..]);

            // Act
            let result = ClientSetup::decode(&mut buf);

            // Assert
            assert!(result.is_err());
        }

        #[test]
        fn decode_rejects_version_count_larger_than_payload() {
            // Arrange
            let bytes_array = [
                224, 0, 0, 0, 0, 0, 0,
                0, // Number of Supported Versions (i): 2^61 as an 8-byte varint
                1, // Supported Version (i)
                0, // Number of Parameters (i)
            ];
            let mut buf = std::io::Cursor::new(&bytes_array[..]);

            // Act
            let result = ClientSetup::decode(&mut buf);

            // Assert
            let error = result.unwrap_err().to_string();
            assert!(
                error.contains("number_of_supported_versions"),
                "unexpected error: {error}"
            );
        }
    }
}
