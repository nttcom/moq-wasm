use crate::modules::variable_integer::read_variable_integer_from_buffer;
use std::any::Any;

use super::moqt_payload::MOQTPayload;
use anyhow::{bail, ensure, Context, Result};
use bytes::BufMut;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::Serialize;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub enum SetupParameter {
    RoleParameter(RoleParameter),
    PathParameter(PathParameter),
    Unknown(u8),
}

impl MOQTPayload for SetupParameter {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let key = SetupParameterType::try_from(u8::try_from(
            read_variable_integer_from_buffer(buf).context("key")?,
        )?);
        if let Err(err) = key {
            tracing::warn!("Unknown SETUP parameter {:#04x}", err.number);
            return Ok(SetupParameter::Unknown(err.number));
        }

        match key? {
            SetupParameterType::Role => {
                let value_length = u8::try_from(read_variable_integer_from_buffer(buf)?)
                    .context("role value length")?;
                ensure!(
                    value_length == 1,
                    "Invalid value length in ROLE parameter {:#04x}",
                    value_length
                );

                let value = RoleCase::try_from(u8::try_from(
                    read_variable_integer_from_buffer(buf).context("role value")?,
                )?);
                if let Err(err) = value {
                    bail!("Invalid value in ROLE parameter {:?}", err);
                }

                Ok(SetupParameter::RoleParameter(RoleParameter::new(value?)))
            }

            // Not implemented as only WebTransport is supported now.
            SetupParameterType::Path => {
                // let value = String::from_utf8(read_variable_bytes_from_buffer(buf)?)?;
                // Ok(SetupParameter::PathParameter(PathParameter::new(value)))

                unimplemented!("Not implemented as only WebTransport is supported.")
            }
        }
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        match self {
            SetupParameter::RoleParameter(param) => {
                buf.put_u8(param.key.into());
                buf.put_u8(0x01);
                buf.put_u8(param.value.into());
            }

            // Not implemented as only WebTransport is supported now.
            SetupParameter::PathParameter(_param) => {
                // buf.put_u8(param.key.into());
                // buf.put_u8(param.value.len() as u8);
                // buf.extend(write_variable_bytes(&param.value.as_bytes().to_vec()));

                unimplemented!("Not implemented as only WebTransport is supported.")
            }
            SetupParameter::Unknown(_) => unimplemented!("Unknown SETUP parameter"),
        }
    }
    /// Method to enable downcasting from MOQTPayload to SetupParameter
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct RoleParameter {
    pub key: SetupParameterType, // 0x00
    pub value_length: u8,        // 0x01
    pub value: RoleCase,
}

impl RoleParameter {
    pub fn new(role: RoleCase) -> Self {
        RoleParameter {
            key: SetupParameterType::Role,
            value_length: 0x01,
            value: role,
        }
    }
}

#[derive(Debug, Clone, Copy, IntoPrimitive, TryFromPrimitive, Serialize, PartialEq)]
#[repr(u8)]
pub enum RoleCase {
    Injection = 0x01,
    Delivery = 0x02,
    Both = 0x03,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct PathParameter {
    pub key: SetupParameterType, // 0x01
    pub value_length: u8,        // tmp
    pub value: String,
}

impl PathParameter {
    pub fn new(value: String) -> Self {
        PathParameter {
            key: SetupParameterType::Path,
            value_length: value.len() as u8,
            value,
        }
    }
}

#[derive(Debug, Clone, Copy, IntoPrimitive, TryFromPrimitive, Serialize, PartialEq)]
#[repr(u8)]
pub enum SetupParameterType {
    Role = 0x00,
    Path = 0x01,
}

#[cfg(test)]
mod success {
    use crate::messages::moqt_payload::MOQTPayload;
    use crate::modules::messages::setup_parameters::{RoleCase, RoleParameter, SetupParameter};
    use bytes::BytesMut;
    #[test]
    fn packetize_role() {
        let role_parameter = RoleParameter::new(RoleCase::Injection);
        let setup_parameter = SetupParameter::RoleParameter(role_parameter);

        let mut buf = bytes::BytesMut::new();
        setup_parameter.packetize(&mut buf);

        let expected_bytes_array = [
            0, // Set Up Parameter Type(Role)
            1, // Value Length
            1, // Role(Injection)
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn depacketize_role() {
        let bytes_array = [
            0, // Set Up Parameter Type(Role)
            1, // Value Length
            2, // Role(Delivery)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_setup_parameter = SetupParameter::depacketize(&mut buf).unwrap();

        let role_parameter = RoleParameter::new(RoleCase::Delivery);
        let expected_setup_parameter = SetupParameter::RoleParameter(role_parameter);
        assert_eq!(depacketized_setup_parameter, expected_setup_parameter);
    }

    #[test]
    fn depacketize_unknown() {
        let bytes_array = [
            2, // Set Up Parameter Type (Unknown)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_setup_parameter = SetupParameter::depacketize(&mut buf);
        assert!(depacketized_setup_parameter.is_ok());
    }
}

#[cfg(test)]
mod failure {
    use crate::messages::moqt_payload::MOQTPayload;
    use crate::modules::messages::setup_parameters::{PathParameter, SetupParameter};
    use bytes::BytesMut;

    #[test]
    fn depacketize_role_invalid_value_length() {
        let bytes_array = [
            0,  // Set Up Parameter Type
            99, // Wrong Value Length
            1,  // Role(Injection)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_setup_parameter = SetupParameter::depacketize(&mut buf);

        assert!(depacketized_setup_parameter.is_err());
    }

    #[test]
    fn depacketize_role_invalid_value() {
        let bytes_array = [
            0,  // Set Up Parameter Type(Role)
            1,  // Value Length
            99, // Role(Wrong)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_setup_parameter = SetupParameter::depacketize(&mut buf);

        assert!(depacketized_setup_parameter.is_err());
    }

    #[test]
    #[should_panic]
    fn packetize_path() {
        let path_parameter = PathParameter::new(String::from("test"));
        let setup_parameter = SetupParameter::PathParameter(path_parameter);

        let mut buf = bytes::BytesMut::new();
        setup_parameter.packetize(&mut buf);
    }

    #[test]
    #[should_panic]
    fn depacketize_path() {
        let bytes_array = [
            1, // Set Up Parameter Type(Path)
            4, // Value Length
            116, 101, 115, 116, // Value("test")
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let _ = SetupParameter::depacketize(&mut buf).unwrap();
    }

    #[test]
    #[should_panic]
    fn packetize_unknown() {
        let setup_parameter = SetupParameter::Unknown(99);

        let mut buf = bytes::BytesMut::new();
        setup_parameter.packetize(&mut buf);
    }
}
