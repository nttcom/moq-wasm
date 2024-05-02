use crate::{
    modules::variable_integer::read_variable_integer_from_buffer,
    variable_bytes::{read_fixed_length_bytes_from_buffer, write_fixed_length_bytes},
    variable_integer::write_variable_integer,
};
use anyhow::Ok;
use num_enum::TryFromPrimitive;

use super::moqt_payload::MOQTPayload;

/// This structure is a parameter that uses a version-specific namespace, unlike Setup parameters,
/// which uses a namespace that is constant across all MoQ Transport versions.
///
/// This structure is referred by messages using parameters other than Setup parameters.
#[derive(Debug, PartialEq)]
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
        if let Err(err) = parameter_type {
            // If it appears in some other type of message, it MUST be ignored.
            tracing::info!("Unknown version specific parameter {:#04x}", err.number);
            return Ok(VersionSpecificParameter::Unknown(err.number));
        }

        match parameter_type? {
            VersionSpecificParameterType::GroupSequence => {
                let _parameter_length = u8::try_from(read_variable_integer_from_buffer(buf)?)?;
                // The value is of type varint.
                let parameter_value = read_variable_integer_from_buffer(buf)?;

                // TODO
                //   If the parameter value is a varint, but the self-encoded length of that
                //   varint does not match the Parameter Length field, the receiver MUST
                //   ignore the parameter using the value in the Parameter Length field.
                // See: https://datatracker.ietf.org/doc/html/draft-ietf-moq-transport-01#name-parameters

                Ok(VersionSpecificParameter::GroupSequence(GroupSequence::new(
                    parameter_value,
                )))
            }
            VersionSpecificParameterType::ObjectSequence => {
                let _parameter_length = u8::try_from(read_variable_integer_from_buffer(buf)?)?;
                // The value is of type varint.
                let parameter_value = read_variable_integer_from_buffer(buf)?;

                // TODO
                //   If the parameter value is a varint, but the self-encoded length of that
                //   varint does not match the Parameter Length field, the receiver MUST
                //   ignore the parameter using the value in the Parameter Length field.
                // See: https://datatracker.ietf.org/doc/html/draft-ietf-moq-transport-01#name-parameters

                Ok(VersionSpecificParameter::ObjectSequence(
                    ObjectSequence::new(parameter_value),
                ))
            }
            VersionSpecificParameterType::AuthorizationInfo => {
                let parameter_length = u8::try_from(read_variable_integer_from_buffer(buf)?)?;
                // The value is an ASCII string.
                let parameter_value = String::from_utf8(read_fixed_length_bytes_from_buffer(
                    buf,
                    parameter_length as usize,
                )?)?;

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
}

#[derive(Debug, Clone, Copy, TryFromPrimitive, PartialEq)]
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

#[derive(Debug, PartialEq)]
pub struct GroupSequence {
    parameter_type: VersionSpecificParameterType,
    length: u8,
    value: u64,
}

impl GroupSequence {
    pub fn new(value: u64) -> Self {
        GroupSequence {
            parameter_type: VersionSpecificParameterType::GroupSequence,
            length: value.to_be_bytes().len() as u8,
            value,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct ObjectSequence {
    parameter_type: VersionSpecificParameterType,
    length: u8,
    value: u64,
}

impl ObjectSequence {
    pub fn new(value: u64) -> Self {
        ObjectSequence {
            parameter_type: VersionSpecificParameterType::ObjectSequence,
            length: value.to_be_bytes().len() as u8,
            value,
        }
    }
}

#[derive(Debug, PartialEq)]
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
        VersionSpecificParameterType,
    };
    use crate::variable_bytes::write_fixed_length_bytes;
    use crate::variable_integer::write_variable_integer;

    #[test]
    fn packetize_group_sequence() {
        let parameter_value = 0x01_u64;
        let parameter_length = parameter_value.to_be_bytes().len() as u8;

        let parameter =
            VersionSpecificParameter::GroupSequence(GroupSequence::new(parameter_value));

        let mut buf = bytes::BytesMut::new();
        parameter.packetize(&mut buf);

        // Parameter Type
        let mut combined_bytes =
            Vec::from((VersionSpecificParameterType::GroupSequence as u8).to_be_bytes());
        // Parameter Length
        combined_bytes.extend(write_variable_integer(parameter_length as u64));
        // Parameter Value
        combined_bytes.extend(write_variable_integer(parameter_value));

        assert_eq!(buf.as_ref(), combined_bytes.as_slice());
    }

    #[test]
    fn packetize_object_sequence() {
        let parameter_value = 0x01_u64;
        let parameter_length = parameter_value.to_be_bytes().len() as u8;

        let parameter =
            VersionSpecificParameter::ObjectSequence(ObjectSequence::new(parameter_value));

        let mut buf = bytes::BytesMut::new();
        parameter.packetize(&mut buf);

        // Parameter Type
        let mut combined_bytes =
            Vec::from((VersionSpecificParameterType::ObjectSequence as u8).to_be_bytes());
        // Parameter Length
        combined_bytes.extend(write_variable_integer(parameter_length as u64));
        // Parameter Value
        combined_bytes.extend(write_variable_integer(parameter_value));

        assert_eq!(buf.as_ref(), combined_bytes.as_slice());
    }

    #[test]
    fn packetize_authorization_info() {
        let parameter_value = "test".to_string();
        let parameter_length = parameter_value.len() as u8;

        let parameter = VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(
            parameter_value.clone(),
        ));

        let mut buf = bytes::BytesMut::new();
        parameter.packetize(&mut buf);

        // Parameter Type
        let mut combined_bytes =
            Vec::from((VersionSpecificParameterType::AuthorizationInfo as u8).to_be_bytes());
        // Parameter Length
        combined_bytes.extend(write_variable_integer(parameter_length as u64));
        // Parameter Value
        combined_bytes.extend(write_fixed_length_bytes(
            &parameter_value.as_bytes().to_vec(),
        ));

        assert_eq!(buf.as_ref(), combined_bytes.as_slice());
    }

    #[test]
    fn depacketize_group_sequence() {
        let parameter_value = 0x01_u64;
        let parameter_length = parameter_value.to_be_bytes().len() as u8;

        let expected_parameter =
            VersionSpecificParameter::GroupSequence(GroupSequence::new(parameter_value));

        // Parameter Type
        let mut combined_bytes =
            Vec::from((VersionSpecificParameterType::GroupSequence as u8).to_be_bytes());
        // Parameter Length
        combined_bytes.extend(write_variable_integer(parameter_length as u64));
        // Parameter Value
        combined_bytes.extend(write_variable_integer(parameter_value));

        let mut buf = bytes::BytesMut::from(combined_bytes.as_slice());
        let depacketized_parameter = VersionSpecificParameter::depacketize(&mut buf).unwrap();

        assert_eq!(depacketized_parameter, expected_parameter);
    }

    #[test]
    fn depacketize_object_sequence() {
        let parameter_value = 0x01_u64;
        let parameter_length = parameter_value.to_be_bytes().len() as u8;

        let expected_parameter =
            VersionSpecificParameter::ObjectSequence(ObjectSequence::new(parameter_value));

        // Parameter Type
        let mut combined_bytes =
            Vec::from((VersionSpecificParameterType::ObjectSequence as u8).to_be_bytes());
        // Parameter Length
        combined_bytes.extend(write_variable_integer(parameter_length as u64));
        // Parameter Value
        combined_bytes.extend(write_variable_integer(parameter_value));

        let mut buf = bytes::BytesMut::from(combined_bytes.as_slice());
        let depacketized_parameter = VersionSpecificParameter::depacketize(&mut buf).unwrap();

        assert_eq!(depacketized_parameter, expected_parameter);
    }

    #[test]
    fn depacketize_authorization_info() {
        let parameter_value = "test".to_string();
        let parameter_length = parameter_value.len() as u8;

        let expected_parameter = VersionSpecificParameter::AuthorizationInfo(
            AuthorizationInfo::new(parameter_value.clone()),
        );

        // Parameter Type
        let mut combined_bytes =
            Vec::from((VersionSpecificParameterType::AuthorizationInfo as u8).to_be_bytes());
        // Parameter Length
        combined_bytes.extend(write_variable_integer(parameter_length as u64));
        // Parameter Value
        combined_bytes.extend(write_fixed_length_bytes(
            &parameter_value.as_bytes().to_vec(),
        ));

        let mut buf = bytes::BytesMut::from(combined_bytes.as_slice());
        let depacketized_parameter = VersionSpecificParameter::depacketize(&mut buf).unwrap();

        assert_eq!(depacketized_parameter, expected_parameter);
    }

    #[test]
    fn depacketize_unknown() {
        // Unknown
        let combined_bytes = Vec::from(write_variable_integer(0x99));

        let mut buf = bytes::BytesMut::from(combined_bytes.as_slice());
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
