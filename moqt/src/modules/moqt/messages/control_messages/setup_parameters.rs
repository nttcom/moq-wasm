use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::messages::{
        control_messages::authorization_token::AuthorizationToken,
        object::key_value_pair::{KeyValuePair, VariantType},
    },
};
use bytes::{Bytes, BytesMut};
use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::Serialize;

#[derive(Debug, Clone, Copy, IntoPrimitive, TryFromPrimitive, Serialize, PartialEq)]
#[repr(u8)]
pub enum SetupParameterType {
    Path = 0x01,
    MaxRequestID = 0x02,
    AuthorizationToken = 0x03,
    MaxAuthTokenCacheSize = 0x04,
    Authority = 0x05,
    MOQTimplementation = 0x07,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetupParameter {
    // 0x01
    pub path: Option<String>,
    // 0x02
    pub max_request_id: u64,
    // 0x03
    pub authorization_token: Vec<AuthorizationToken>,
    // 0x04
    pub max_auth_token_cache_size: Option<u64>,
    // 0x05
    pub authority: Option<String>,
    // 0x07
    pub moq_implementation: Option<String>,
}

impl SetupParameter {
    pub(crate) fn decode(buf: &mut BytesMut) -> Option<Self> {
        let mut kv_pairs = Vec::new();
        let number_of_parameters = buf
            .try_get_varint()
            .log_context("number of parameters")
            .ok()?;
        for _ in 0..number_of_parameters {
            let key_value_pair = KeyValuePair::decode(buf)?;
            kv_pairs.push(key_value_pair);
        }
        let path =
            kv_pairs
                .iter()
                .find(|kv_pair| kv_pair.key == 0x01)
                .map(|kv_pair| match &kv_pair.value {
                    VariantType::Odd(value) => String::from_utf8(value.to_vec()).unwrap(),
                    VariantType::Even(_) => unreachable!(),
                });
        let max_request_id = kv_pairs
            .iter()
            .find(|kv_pair| kv_pair.key == 0x02)
            .map(|kv_pair| match kv_pair.value {
                VariantType::Odd(_) => unreachable!(),
                VariantType::Even(value) => value,
            })
            .unwrap_or(0);
        let authorization_token = kv_pairs
            .iter()
            .filter(|kv_pair| kv_pair.key == 0x03)
            .filter_map(|kv_pair| match &kv_pair.value {
                VariantType::Odd(value) => {
                    let mut value = BytesMut::from(value.iter().as_slice());
                    AuthorizationToken::decode(&mut value)
                }
                VariantType::Even(_) => unreachable!(),
            })
            .collect();
        let max_auth_token_cache_size =
            kv_pairs
                .iter()
                .find(|kv_pair| kv_pair.key == 0x04)
                .map(|kv_pair| match kv_pair.value {
                    VariantType::Odd(_) => unreachable!(),
                    VariantType::Even(value) => value,
                });
        let authority = kv_pairs
            .iter()
            .find(|kv_pair| kv_pair.key == 0x05)
            .map(|kv_pair| match &kv_pair.value {
                VariantType::Odd(value) => String::from_utf8(value.to_vec()).unwrap(),
                VariantType::Even(_) => unreachable!(),
            });
        let moq_implementation =
            kv_pairs
                .iter()
                .find(|kv_pair| kv_pair.key == 0x07)
                .map(|kv_pair| match &kv_pair.value {
                    VariantType::Odd(value) => String::from_utf8(value.to_vec()).unwrap(),
                    VariantType::Even(_) => unreachable!(),
                });

        Some(SetupParameter {
            path,
            max_request_id,
            authorization_token,
            max_auth_token_cache_size,
            authority,
            moq_implementation,
        })
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        let mut number_of_parameters = 0;

        if let Some(path) = &self.path {
            buf.unsplit(
                KeyValuePair {
                    key: SetupParameterType::Path as u64,
                    value: VariantType::Odd(Bytes::from(path.clone().into_bytes())),
                }
                .encode(),
            );
            number_of_parameters += 1;
        }
        buf.unsplit(
            KeyValuePair {
                key: SetupParameterType::MaxRequestID as u64,
                value: VariantType::Even(self.max_request_id),
            }
            .encode(),
        );
        number_of_parameters += 1;
        for authorization_token in &self.authorization_token {
            let auth_token = authorization_token.encode();
            buf.unsplit(
                KeyValuePair {
                    key: SetupParameterType::AuthorizationToken as u64,
                    value: VariantType::Odd(auth_token.into()),
                }
                .encode(),
            );
            number_of_parameters += 1;
        }
        if let Some(max_auth_token_cache_size) = &self.max_auth_token_cache_size {
            buf.unsplit(
                KeyValuePair {
                    key: SetupParameterType::MaxAuthTokenCacheSize as u64,
                    value: VariantType::Even(*max_auth_token_cache_size),
                }
                .encode(),
            );
            number_of_parameters += 1;
        }
        if let Some(authority) = &self.authority {
            buf.unsplit(
                KeyValuePair {
                    key: SetupParameterType::Authority as u64,
                    value: VariantType::Odd(Bytes::from(authority.clone().into_bytes())),
                }
                .encode(),
            );
            number_of_parameters += 1;
        }
        if let Some(moq_implementation) = &self.moq_implementation {
            buf.unsplit(
                KeyValuePair {
                    key: SetupParameterType::MOQTimplementation as u64,
                    value: VariantType::Odd(Bytes::from(moq_implementation.clone().into_bytes())),
                }
                .encode(),
            );
            number_of_parameters += 1;
        }
        let mut result_bytes = BytesMut::new();
        result_bytes.put_varint(number_of_parameters);
        result_bytes.unsplit(buf);
        result_bytes
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use bytes::{Bytes, BytesMut};

        use crate::modules::moqt::messages::control_messages::{
            authorization_token::AuthorizationToken, setup_parameters::SetupParameter,
        };

        #[test]
        fn encode() {
            let token = AuthorizationToken::Register {
                token_alias: 1,
                token_type: 2,
                token_value: Bytes::from(vec![b't', b'o', b'k', b'e', b'n']),
            };
            let setup_parameter = SetupParameter {
                max_request_id: 2000,
                path: Some("path".to_string()),
                authorization_token: vec![token],
                max_auth_token_cache_size: Some(10),
                authority: Some("auth".to_string()),
                moq_implementation: Some("MOQ-WASM".to_string()),
            };

            let buf = setup_parameter.encode();

            let expected_bytes_array = [
                6, // Number of Parameters (i)
                1, // Parameter Type (i): Type(Path)
                4, // Parameter Type (i): Length
                b'p', b'a', b't', b'h', // Parameter Type (i): Value("path")
                2,    // Max Request ID
                71,   // Parameter Value (..): Length(01 of 2MSB)
                208,  // Parameter Value (..): Value(2000) in 62bit
                3,    // Parameter Type(i): Authorization token
                8,    // Parameter Type(i): Length
                1,    // Parameter Type(i): Alias Type(i)
                1,    // Parameter Type(i): Token alias(i)
                2,    // Parameter Type(i): Token Type(i)
                b't', b'o', b'k', b'e', b'n', // Parameter Type(i): Token(..)
                4,    // Parameter Type(i): Length
                10,   // Parameter Type(i): Value
                5,    // Parameter Type(i): Authority
                4,    // Parameter Type(i): Length
                b'a', b'u', b't', b'h', // Parameter Type(i): Value("auth")
                7,    // Parameter Type(i): MOQT-IMPLEMENTATION
                8,    // Parameter Type(i): Length
                77, 79, 81, 45, 87, 65, 83, 77, // Parameter Type(i): Value("MOQ-WASM")
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn decode() {
            let bytes_array = [
                6, // Number of Parameters (i)
                1, // Parameter Type (i): Type(Path)
                4, // Parameter Type (i): Length
                b'p', b'a', b't', b'h', // Parameter Type (i): Value("path")
                2,    // Max Request ID
                71,   // Parameter Value (..): Length(01 of 2MSB)
                208,  // Parameter Value (..): Value(2000) in 62bit
                3,    // Parameter Type(i): Authorization token
                8,    // Parameter Type(i): Length
                1,    // Parameter Type(i): Alias Type(i)
                1,    // Parameter Type(i): Token alias(i)
                2,    // Parameter Type(i): Token Type(i)
                b't', b'o', b'k', b'e', b'n', // Parameter Type(i): Token(..)
                4,    // Parameter Type(i): Length
                10,   // Parameter Type(i): Value
                5,    // Parameter Type(i): Authority
                4,    // Parameter Type(i): Length
                b'a', b'u', b't', b'h', // Parameter Type(i): Value("auth")
                7,    // Parameter Type(i): MOQT-IMPLEMENTATION
                8,    // Parameter Type(i): Length
                77, 79, 81, 45, 87, 65, 83, 77, // Parameter Type(i): Value("MOQ-WASM")
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let depacketized_setup_parameter = SetupParameter::decode(&mut buf).unwrap();
            assert_eq!(depacketized_setup_parameter.max_request_id, 2000);
            assert_eq!(depacketized_setup_parameter.path, Some("path".to_string()));
            assert_eq!(depacketized_setup_parameter.authorization_token.len(), 1);
            assert_eq!(
                depacketized_setup_parameter.max_auth_token_cache_size,
                Some(10)
            );
            assert_eq!(
                depacketized_setup_parameter.authority,
                Some("auth".to_string())
            );
            assert_eq!(
                depacketized_setup_parameter.moq_implementation,
                Some("MOQ-WASM".to_string())
            );
        }

        // #[test]
        // fn depacketize_unknown() {
        //     let bytes_array = [
        //         3, // Parameter Type (i): Type(Unknown)
        //     ];
        //     let mut buf = BytesMut::with_capacity(bytes_array.len());
        //     buf.extend_from_slice(&bytes_array);
        //     let depacketized_setup_parameter = SetupParameter::decode(&mut buf);
        //     assert!(depacketized_setup_parameter.is_ok());
        // }
    }

    // mod failure {
    //     use crate::modules::moqt::messages::{
    //         control_messages::setup_parameters::{Path, SetupParameter},
    //         moqt_payload::MOQTPayload,
    //     };
    //     use bytes::BytesMut;

    //     #[test]
    //     #[should_panic]
    //     fn packetize_path() {
    //         let path_parameter = Path::new(String::from("test"));
    //         let setup_parameter = SetupParameter::Path(path_parameter);

    //         let mut buf = BytesMut::new();
    //         setup_parameter.packetize(&mut buf);
    //     }

    //     #[test]
    //     #[should_panic]
    //     fn packetize_unknown() {
    //         let setup_parameter = SetupParameter::Unknown(99);

    //         let mut buf = BytesMut::new();
    //         setup_parameter.packetize(&mut buf);
    //     }

    //     #[test]
    //     #[should_panic]
    //     fn depacketize_path() {
    //         let bytes_array = [
    //             1, // Parameter Type (i): Type(Path)
    //             4, // Parameter Type (i): Length
    //             116, 101, 115, 116, // Parameter Type (i): Value("test")
    //         ];
    //         let mut buf = BytesMut::with_capacity(bytes_array.len());
    //         buf.extend_from_slice(&bytes_array);
    //         let _ = SetupParameter::decode(&mut buf).unwrap();
    //     }

    //     #[test]
    //     fn depacketize_max_subscribe_id_invalid_length() {
    //         let bytes_array = [
    //             2, // Parameter Type (i): Type(MaxSubscribeID)
    //             2, // Parameter Length (i)
    //             1, // Parameter Value (..)
    //         ];
    //         let mut buf = BytesMut::with_capacity(bytes_array.len());
    //         buf.extend_from_slice(&bytes_array);
    //         let depacketized_setup_parameter = SetupParameter::decode(&mut buf);

    //         assert!(depacketized_setup_parameter.is_err());
    //     }
    // }
}
