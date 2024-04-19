use crate::{
    modules::variable_integer::read_variable_integer_from_buffer,
    variable_bytes::{read_fixed_length_bytes_from_buffer, write_variable_bytes},
    variable_integer::write_variable_integer,
};
use anyhow::Ok;

use super::moqt_payload::MOQTPayload;

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum ParameterType {
    GroupSequence = 0x00,
    ObjectSequence = 0x01,
    AuthorizationInfo = 0x02,
}
impl From<ParameterType> for u64 {
    fn from(parameter_type: ParameterType) -> Self {
        parameter_type as u64
    }
}

#[derive(Debug)]
pub struct Parameter {
    parameter_type: ParameterType,
    length: u8,
    value: String,
}

impl Parameter {
    pub fn new(parameter_type: ParameterType, length: u8, value: String) -> Self {
        Parameter {
            parameter_type,
            length,
            value,
        }
    }
}

impl MOQTPayload for Parameter {
    fn depacketize(buf: &mut bytes::BytesMut) -> anyhow::Result<Self> {
        // u8からParameterTypeに変換
        let parameter_type = match u8::try_from(read_variable_integer_from_buffer(buf)?)? {
            0x00 => ParameterType::GroupSequence,
            0x01 => ParameterType::ObjectSequence,
            0x02 => ParameterType::AuthorizationInfo,
            _ => return Err(anyhow::anyhow!("Unknown ParameterType")),
        };

        let parameter_length = u8::try_from(read_variable_integer_from_buffer(buf)?)?;
        let parameter_value = String::from_utf8(read_fixed_length_bytes_from_buffer(
            buf,
            parameter_length as usize,
        )?)?;

        Ok(Parameter {
            parameter_type,
            length: parameter_length,
            value: parameter_value,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        /*
            Parameter {
                Parameter Type (i),
                Parameter Length (i),
                Parameter Value (..),
            }
        */
        buf.extend(write_variable_integer(u64::from(self.parameter_type)));
        buf.extend(write_variable_integer(self.length as u64));
        buf.extend(write_variable_bytes(&self.value.as_bytes().to_vec()));
    }
}
