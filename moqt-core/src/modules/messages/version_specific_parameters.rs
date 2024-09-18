use super::moqt_payload::MOQTPayload;
use crate::{
    modules::variable_integer::read_variable_integer_from_buffer,
    variable_bytes::{
        convert_bytes_to_integer, read_variable_bytes_from_buffer, write_fixed_length_bytes,
    },
    variable_integer::{get_length_from_variable_integer_first_byte, write_variable_integer},
};
use anyhow::Ok;
use num_enum::TryFromPrimitive;
use serde::Serialize;
use std::any::Any;

/// This structure is a parameter that uses a version-specific namespace, unlike Setup parameters,
/// which uses a namespace that is constant across all MoQ Transport versions.
///
/// This structure is referred by messages using parameters other than Setup parameters.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub enum VersionSpecificParameter {
    GroupSequence(GroupSequence),
    ObjectSequence(ObjectSequence),
    AuthorizationInfo(AuthorizationInfo),
    Unknown(u8),
}

impl MOQTPayload for VersionSpecificParameter {
    fn depacketize(buf: &mut bytes::BytesMut) -> anyhow::Result<Self> {
        let parameter_type = VersionSpecificParameterType::try_from(u8::try_from(
            read_variable_integer_from_buffer(buf)?,
        )?);
        let parameter_value = read_variable_bytes_from_buffer(buf)?;

        if let Err(err) = parameter_type {
            // If it appears in some other type of message, it MUST be ignored.
            tracing::warn!("Unknown version specific parameter {:#04x}", err.number);
            return Ok(VersionSpecificParameter::Unknown(err.number));
        }

        match parameter_type? {
            VersionSpecificParameterType::GroupSequence => {
                // The value is of type varint.
                let parameter_value: u64 = convert_bytes_to_integer(parameter_value)?;

                Ok(VersionSpecificParameter::GroupSequence(GroupSequence::new(
                    parameter_value,
                )))
            }
            VersionSpecificParameterType::ObjectSequence => {
                // The value is of type varint.
                let parameter_value: u64 = convert_bytes_to_integer(parameter_value)?;

                Ok(VersionSpecificParameter::ObjectSequence(
                    ObjectSequence::new(parameter_value),
                ))
            }
            VersionSpecificParameterType::AuthorizationInfo => {
                // The value is an ASCII string.
                let parameter_value: String = String::from_utf8(parameter_value)?;

                Ok(VersionSpecificParameter::AuthorizationInfo(
                    AuthorizationInfo::new(parameter_value),
                ))
            }
        }
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        /*
            Parameter {
                Parameter Type (i),
                Parameter Length (i),
                Parameter Value (..),
            }
        */

        match self {
            VersionSpecificParameter::GroupSequence(param) => {
                buf.extend(write_variable_integer(u64::from(param.parameter_type)));
                buf.extend(write_variable_integer(param.length as u64));
                //   The value is of type varint.
                buf.extend(write_variable_integer(param.value));
            }
            VersionSpecificParameter::ObjectSequence(param) => {
                buf.extend(write_variable_integer(u64::from(param.parameter_type)));
                buf.extend(write_variable_integer(param.length as u64));
                //   The value is of type varint.
                buf.extend(write_variable_integer(param.value));
            }
            VersionSpecificParameter::AuthorizationInfo(param) => {
                buf.extend(write_variable_integer(u64::from(param.parameter_type)));
                buf.extend(write_variable_integer(param.length as u64));
                //   The value is an ASCII string.
                buf.extend(write_fixed_length_bytes(&param.value.as_bytes().to_vec()));
            }
            VersionSpecificParameter::Unknown(_) => {
                unimplemented!("Unknown version specific parameter")
            }
        }
    }
    /// Method to enable downcasting from MOQTPayload to VersionSpecificParameter
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Serialize, Clone, Copy, TryFromPrimitive, PartialEq)]
#[repr(u8)]
pub enum VersionSpecificParameterType {
    GroupSequence = 0x00,
    ObjectSequence = 0x01,
    AuthorizationInfo = 0x02,
}

impl From<VersionSpecificParameterType> for u64 {
    fn from(parameter_type: VersionSpecificParameterType) -> Self {
        parameter_type as u64
    }
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct GroupSequence {
    parameter_type: VersionSpecificParameterType,
    length: u8,
    value: u64,
}

impl GroupSequence {
    pub fn new(value: u64) -> Self {
        let first_byte = (value & 0xFF) as u8; // 0xFF: Bit mask to get the first byte
        let length = get_length_from_variable_integer_first_byte(first_byte);

        GroupSequence {
            parameter_type: VersionSpecificParameterType::GroupSequence,
            length,
            value,
        }
    }
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ObjectSequence {
    parameter_type: VersionSpecificParameterType,
    length: u8,
    value: u64,
}

impl ObjectSequence {
    pub fn new(value: u64) -> Self {
        let first_byte = (value & 0xFF) as u8; // 0xFF: Bit mask to get the first byte
        let length = get_length_from_variable_integer_first_byte(first_byte);

        ObjectSequence {
            parameter_type: VersionSpecificParameterType::ObjectSequence,
            length,
            value,
        }
    }
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct AuthorizationInfo {
    parameter_type: VersionSpecificParameterType,
    length: u8,
    value: String,
}

impl AuthorizationInfo {
    pub fn new(value: String) -> Self {
        AuthorizationInfo {
            parameter_type: VersionSpecificParameterType::AuthorizationInfo,
            length: value.len() as u8,
            value,
        }
    }
}

#[cfg(test)]
mod success {
    use crate::modules::messages::moqt_payload::MOQTPayload;
    use crate::modules::messages::version_specific_parameters::{
        AuthorizationInfo, GroupSequence, ObjectSequence, VersionSpecificParameter,
    };

    #[test]
    fn packetize_group_sequence() {
        let parameter_value = 0x01;
        let parameter =
            VersionSpecificParameter::GroupSequence(GroupSequence::new(parameter_value));
        let mut buf = bytes::BytesMut::new();
        parameter.packetize(&mut buf);

        let expected_bytes_array = [
            0, // Parameter Type (i): GroupSequence
            1, // Parameter Length (i)
            1, // Parameter Value (..)
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn packetize_object_sequence() {
        let parameter_value = 0x01;
        let parameter =
            VersionSpecificParameter::ObjectSequence(ObjectSequence::new(parameter_value));
        let mut buf = bytes::BytesMut::new();
        parameter.packetize(&mut buf);

        let expected_bytes_array = [
            1, // Parameter Type (i): ObjectSequence
            1, // Parameter Length (i)
            1, // Parameter Value (..)
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn packetize_authorization_info() {
        let parameter_value = "test".to_string();
        let parameter = VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(
            parameter_value.clone(),
        ));
        let mut buf = bytes::BytesMut::new();
        parameter.packetize(&mut buf);

        let expected_bytes_array = [
            2, // Parameter Type (i): AuthorizationInfo
            4, // Parameter Length (i)
            116, 101, 115, 116, // Parameter Value (..): test
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn depacketize_group_sequence() {
        let bytes_array = [
            0, // Parameter Type (i): GroupSequence
            1, // Parameter Length
            1, // Parameter Value (..)
        ];
        let mut buf = bytes::BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_parameter = VersionSpecificParameter::depacketize(&mut buf).unwrap();

        let expected_parameter = VersionSpecificParameter::GroupSequence(GroupSequence::new(1));
        assert_eq!(depacketized_parameter, expected_parameter);
    }

    #[test]
    fn depacketize_object_sequence() {
        let bytes_array = [
            1, // Parameter Type (i): ObjectSequence
            1, // Parameter Length
            1, // Parameter Value (..)
        ];
        let mut buf = bytes::BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_parameter = VersionSpecificParameter::depacketize(&mut buf).unwrap();

        let expected_parameter = VersionSpecificParameter::ObjectSequence(ObjectSequence::new(1));
        assert_eq!(depacketized_parameter, expected_parameter);
    }

    #[test]
    fn depacketize_authorization_info() {
        let bytes_array = [
            2, // Parameter Type (i): AuthorizationInfo
            4, // Parameter Length
            116, 101, 115, 116, // Parameter Value (..): test
        ];
        let mut buf = bytes::BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_parameter = VersionSpecificParameter::depacketize(&mut buf).unwrap();

        let expected_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        assert_eq!(depacketized_parameter, expected_parameter);
    }

    #[test]
    fn depacketize_unknown() {
        let bytes_array = [
            64, // Parameter Type (i): Length. 64(0b01000000) equals to Length=2 and Usable Bits is 14bit in 2MSB.
            99, // Parameter Type (i): Unknown. this value is represented in 14bit.
            4,  // Parameter Length (i)
            116, 101, 115, 116, // PParameter Value (..): test"
        ];
        let mut buf = bytes::BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_version_specific_parameter =
            VersionSpecificParameter::depacketize(&mut buf);

        assert!(depacketized_version_specific_parameter.is_ok());
    }
}

#[cfg(test)]
mod failure {
    use crate::modules::messages::moqt_payload::MOQTPayload;
    use crate::modules::messages::version_specific_parameters::VersionSpecificParameter;

    #[test]
    #[should_panic]
    fn packetize_unknown() {
        let version_specific_parameter = VersionSpecificParameter::Unknown(99);

        let mut buf = bytes::BytesMut::new();
        version_specific_parameter.packetize(&mut buf);
    }
}
