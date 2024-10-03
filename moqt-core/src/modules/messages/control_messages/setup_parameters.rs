use crate::modules::variable_integer::{read_variable_integer_from_buffer, write_variable_integer};
use std::any::Any;

use crate::messages::moqt_payload::MOQTPayload;
use anyhow::{bail, ensure, Context, Result};
use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::Serialize;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub enum SetupParameter {
    Role(Role),
    Path(Path),
    MaxSubscribeID(MaxSubscribeID),
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
                let length = u8::try_from(read_variable_integer_from_buffer(buf)?)
                    .context("role value length")?;

                // TODO: return TerminationError
                ensure!(
                    length == 1,
                    "Invalid value length in ROLE parameter {:#04x}",
                    length
                );

                let value = RoleCase::try_from(u8::try_from(
                    read_variable_integer_from_buffer(buf).context("role value")?,
                )?);
                if let Err(err) = value {
                    bail!("Invalid value in ROLE parameter {:?}", err);
                }

                Ok(SetupParameter::Role(Role::new(value?)))
            }

            // Not implemented as only WebTransport is supported now.
            SetupParameterType::Path => {
                // let value = String::from_utf8(read_variable_bytes_from_buffer(buf)?)?;
                // Ok(SetupParameter::Path(Path::new(value)))

                unimplemented!("Not implemented as only WebTransport is supported.")
            }

            SetupParameterType::MaxSubscribeID => {
                let length = read_variable_integer_from_buffer(buf)?;
                let value = read_variable_integer_from_buffer(buf).context("max subscribe id")?;

                if write_variable_integer(value).len() as u64 != length {
                    // TODO: return TerminationError
                    bail!("Invalid value length in MAX_SUBSCRIBE_ID parameter");
                }

                Ok(SetupParameter::MaxSubscribeID(MaxSubscribeID::new(value)))
            }
        }
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        match self {
            SetupParameter::Role(param) => {
                buf.extend(write_variable_integer(u8::from(param.key) as u64));
                buf.extend(write_variable_integer(param.length));
                //   The value is of type varint.
                buf.extend(write_variable_integer(u8::from(param.value) as u64));
            }

            // Not implemented as only WebTransport is supported now.
            SetupParameter::Path(_param) => {
                unimplemented!("Not implemented as only WebTransport is supported.")
            }

            SetupParameter::MaxSubscribeID(param) => {
                buf.extend(write_variable_integer(u8::from(param.key) as u64));
                buf.extend(write_variable_integer(param.length));
                //   The value is of type varint (from MAX_SUBSCRIBE_ID message format).
                buf.extend(write_variable_integer(param.value));
            }

            SetupParameter::Unknown(_) => unimplemented!("Unknown SETUP parameter"),
        }
    }
    /// Method to enable downcasting from MOQTPayload to SetupParameter
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone, Copy, IntoPrimitive, TryFromPrimitive, Serialize, PartialEq)]
#[repr(u8)]
pub enum SetupParameterType {
    Role = 0x00,
    Path = 0x01,
    MaxSubscribeID = 0x02,
}

#[derive(Debug, Clone, Copy, IntoPrimitive, TryFromPrimitive, Serialize, PartialEq)]
#[repr(u8)]
pub enum RoleCase {
    Publisher = 0x01,
    Subscriber = 0x02,
    PubSub = 0x03,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Role {
    pub key: SetupParameterType,
    pub length: u64,
    pub value: RoleCase,
}

impl Role {
    pub fn new(role: RoleCase) -> Self {
        Role {
            key: SetupParameterType::Role,
            length: 0x01,
            value: role,
        }
    }
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Path {
    pub key: SetupParameterType,
    pub length: u64,
    pub value: String,
}

impl Path {
    pub fn new(path: String) -> Self {
        Path {
            key: SetupParameterType::Path,
            length: path.len() as u64,
            value: path,
        }
    }
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct MaxSubscribeID {
    pub key: SetupParameterType,
    pub length: u64,
    pub value: u64,
}

impl MaxSubscribeID {
    pub fn new(max_subscribe_id: u64) -> Self {
        let length = write_variable_integer(max_subscribe_id).len() as u64;
        MaxSubscribeID {
            key: SetupParameterType::MaxSubscribeID,
            length,
            value: max_subscribe_id,
        }
    }
}

#[cfg(test)]
mod success {
    use crate::messages::moqt_payload::MOQTPayload;
    use crate::modules::messages::control_messages::setup_parameters::{
        MaxSubscribeID, Role, RoleCase, SetupParameter,
    };
    use bytes::BytesMut;

    #[test]
    fn packetize_role() {
        let role_parameter = Role::new(RoleCase::Publisher);
        let setup_parameter = SetupParameter::Role(role_parameter);

        let mut buf = bytes::BytesMut::new();
        setup_parameter.packetize(&mut buf);

        let expected_bytes_array = [
            0, // Parameter Type (i): Role
            1, // Parameter Length (i)
            1, // Parameter Value (..): Role(Publisher)
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn depacketize_role() {
        let bytes_array = [
            0, // Parameter Type (i): Role
            1, // Parameter Length (i)
            2, // Parameter Value (..): Role(Subscriber)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_setup_parameter = SetupParameter::depacketize(&mut buf).unwrap();

        let role_parameter = Role::new(RoleCase::Subscriber);
        let expected_setup_parameter = SetupParameter::Role(role_parameter);
        assert_eq!(depacketized_setup_parameter, expected_setup_parameter);
    }

    #[test]
    fn packetize_max_subscribe_id() {
        let max_subscribe_id = MaxSubscribeID::new(2000);
        let setup_parameter = SetupParameter::MaxSubscribeID(max_subscribe_id);

        let mut buf = bytes::BytesMut::new();
        setup_parameter.packetize(&mut buf);

        let expected_bytes_array = [
            2,   // Parameter Type (i): Type(MaxSubscribeID)
            2,   // Parameter Length (i)
            71,  // Parameter Value (..): Length(01 of 2MSB)
            208, // Parameter Value (..): Value(2000) in 62bit
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn depacketize_max_subscribe_id() {
        let bytes_array = [
            2,   // Parameter Type (i): Type(MaxSubscribeID)
            2,   // Parameter Length (i)
            75,  // Parameter Value (..): Length(01 of 2MSB)
            184, // Parameter Value (..): Value(3000) in 62bit
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_setup_parameter = SetupParameter::depacketize(&mut buf).unwrap();

        let max_subscribe_id = MaxSubscribeID::new(3000);
        let expected_setup_parameter = SetupParameter::MaxSubscribeID(max_subscribe_id);
        assert_eq!(depacketized_setup_parameter, expected_setup_parameter);
    }

    #[test]
    fn depacketize_unknown() {
        let bytes_array = [
            3, // Parameter Type (i): Type(Unknown)
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
    use crate::modules::messages::control_messages::setup_parameters::{Path, SetupParameter};
    use bytes::BytesMut;

    #[test]
    fn depacketize_role_invalid_length() {
        let bytes_array = [
            0,  // Parameter Type (i): Type(Role)
            99, // Parameter Type (i): Length(Wrong)
            1,  // Parameter Type (i): Role(Publisher)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_setup_parameter = SetupParameter::depacketize(&mut buf);

        assert!(depacketized_setup_parameter.is_err());
    }

    #[test]
    fn depacketize_role_invalid_value() {
        let bytes_array = [
            0,  // Parameter Type (i): Type(Role)
            1,  // Parameter Type (i): Length
            99, // Parameter Type (i): Role(Wrong)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_setup_parameter = SetupParameter::depacketize(&mut buf);

        assert!(depacketized_setup_parameter.is_err());
    }

    #[test]
    #[should_panic]
    fn packetize_path() {
        let path_parameter = Path::new(String::from("test"));
        let setup_parameter = SetupParameter::Path(path_parameter);

        let mut buf = bytes::BytesMut::new();
        setup_parameter.packetize(&mut buf);
    }

    #[test]
    #[should_panic]
    fn depacketize_path() {
        let bytes_array = [
            1, // Parameter Type (i): Type(Path)
            4, // Parameter Type (i): Length
            116, 101, 115, 116, // Parameter Type (i): Value("test")
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let _ = SetupParameter::depacketize(&mut buf).unwrap();
    }

    #[test]
    fn depacketize_max_subscribe_id_invalid_length() {
        let bytes_array = [
            2, // Parameter Type (i): Type(MaxSubscribeID)
            2, // Parameter Length (i)
            1, // Parameter Value (..)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_setup_parameter = SetupParameter::depacketize(&mut buf);

        assert!(depacketized_setup_parameter.is_err());
    }

    #[test]
    #[should_panic]
    fn packetize_unknown() {
        let setup_parameter = SetupParameter::Unknown(99);

        let mut buf = bytes::BytesMut::new();
        setup_parameter.packetize(&mut buf);
    }
}
