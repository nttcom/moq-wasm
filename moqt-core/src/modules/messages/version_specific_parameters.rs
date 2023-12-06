use anyhow::Ok;

use crate::modules::{
    variable_bytes::read_variable_bytes_from_buffer,
    variable_integer::read_variable_integer_from_buffer,
};

use super::moqt_payload::MOQTPayload;

pub enum TrackRequestParameter {
    AuthorizationInfo(AuthorizationInfoParameter),
    Unknown(u8),
}

impl MOQTPayload for TrackRequestParameter {
    fn depacketize(buf: &mut bytes::BytesMut) -> anyhow::Result<Self> {
        let parameter_key = u8::try_from(read_variable_integer_from_buffer(buf)?)?;

        match parameter_key {
            0x02 => {
                // AuthorizationInfo
                let parameter_length = u8::try_from(read_variable_integer_from_buffer(buf)?)?;
                let parameter_value = String::from_utf8(read_variable_bytes_from_buffer(buf)?)?;

                Ok(TrackRequestParameter::AuthorizationInfo(
                    AuthorizationInfoParameter {
                        parameter_key,
                        parameter_length,
                        parameter_value,
                    },
                ))
            }
            _ => {
                // unknown parameter
                Ok(TrackRequestParameter::Unknown(parameter_key))
            }
        }
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        todo!()
    }
}

// for SUBSCRIBE REQUEST and ANNOUNCE
pub struct AuthorizationInfoParameter {
    parameter_key: u8, // 0x02
    parameter_length: u8,
    parameter_value: String,
}
