use crate::modules::moqt::messages::{
    moqt_payload::MOQTPayload,
    variable_bytes::read_variable_bytes_from_buffer,
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
use anyhow::{Context, Result, bail};
use bytes::BytesMut;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::Serialize;
use std::any::Any;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub enum SetupParameter {
    Path(Path),
    MaxSubscribeID(MaxSubscribeID),
    MOQTimplementation(MOQTimplementation),
    Unknown(u8),
}

impl MOQTPayload for SetupParameter {
    fn depacketize(buf: &mut BytesMut) -> Result<Self> {
        let key = SetupParameterType::try_from(u8::try_from(
            read_variable_integer_from_buffer(buf).context("key")?,
        )?);
        if let Err(err) = key {
            tracing::warn!("Unknown SETUP parameter {:#04x}", err.number);
            return Ok(SetupParameter::Unknown(err.number));
        }

        match key? {
            // Not implemented as only WebTransport is supported now.
            SetupParameterType::Path => {
                // let value = String::from_utf8(read_variable_bytes_from_buffer(buf)?)?;
                // Ok(SetupParameter::Path(Path::new(value)))

                unimplemented!("Not implemented as only WebTransport is supported.")
            }
            SetupParameterType::MaxSubscribeID => {
                let value = read_variable_integer_from_buffer(buf).context("max subscribe id")?;

                Ok(SetupParameter::MaxSubscribeID(MaxSubscribeID::new(value)))
            }
            SetupParameterType::MOQTimplementation => {
                let value = String::from_utf8(read_variable_bytes_from_buffer(buf)?)?;
                Ok(SetupParameter::MOQTimplementation(MOQTimplementation::new(value)))
            }
        }
    }

    fn packetize(&self, buf: &mut BytesMut) {
        match self {
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
            SetupParameter::MOQTimplementation(param) => {
                buf.extend(write_variable_integer(u8::from(param.key) as u64));
                buf.extend(write_variable_integer(param.length));
                buf.extend(param.value.as_bytes());
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
    Path = 0x01,
    MaxSubscribeID = 0x02,
    MOQTimplementation = 0x07,
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

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct MOQTimplementation {
    pub key: SetupParameterType,
    pub length: u64,
    pub value: String,
}

impl MOQTimplementation {
    pub fn new(value: String) -> Self {
        let length = value.len() as u64;
        MOQTimplementation {
            key: SetupParameterType::MOQTimplementation,
            length,
            value,
        }
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use bytes::BytesMut;

        use crate::modules::moqt::messages::{
            control_messages::setup_parameters::{MaxSubscribeID, SetupParameter},
            moqt_payload::MOQTPayload,
        };

        #[test]
        fn packetize_max_subscribe_id() {
            let max_subscribe_id = MaxSubscribeID::new(2000);
            let setup_parameter = SetupParameter::MaxSubscribeID(max_subscribe_id);

            let mut buf = BytesMut::new();
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

    mod failure {
        use crate::modules::moqt::messages::{
            control_messages::setup_parameters::{Path, SetupParameter},
            moqt_payload::MOQTPayload,
        };
        use bytes::BytesMut;

        #[test]
        #[should_panic]
        fn packetize_path() {
            let path_parameter = Path::new(String::from("test"));
            let setup_parameter = SetupParameter::Path(path_parameter);

            let mut buf = BytesMut::new();
            setup_parameter.packetize(&mut buf);
        }

        #[test]
        #[should_panic]
        fn packetize_unknown() {
            let setup_parameter = SetupParameter::Unknown(99);

            let mut buf = BytesMut::new();
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
    }
}
