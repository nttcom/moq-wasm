use crate::modules::variable_integer::read_variable_integer_from_buffer;

use super::moqt_payload::MOQTPayload;
use anyhow::{bail, ensure, Result};
use num_enum::{IntoPrimitive, TryFromPrimitive};

// TODO FIXME: そもそもvalueだけ持たせれば後ろの個別のstructはいらないのでは?
pub(crate) enum SetupParameter {
    RoleParameter(RoleParameter),
    PathParameter(PathParameter),
    Unknown(u8),
}

impl MOQTPayload for SetupParameter {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let key =
            SetupParameterType::try_from(u8::try_from(read_variable_integer_from_buffer(buf)?)?);
        if let Err(err) = key {
            tracing::info!("Unknown SETUP parameter {:#04x}", err.number);
            return Ok(SetupParameter::Unknown(err.number));
        }

        match key? {
            SetupParameterType::Role => {
                let value_length = u8::try_from(read_variable_integer_from_buffer(buf)?)?;
                ensure!(
                    value_length == 1,
                    "Invalid value length in ROLE parameter {:#04x}",
                    value_length
                );

                let value =
                    RoleCase::try_from(u8::try_from(read_variable_integer_from_buffer(buf)?)?);
                if let Err(err) = value {
                    bail!("Invalid value in ROLE parameter {:?}", err);
                }

                Ok(SetupParameter::RoleParameter(RoleParameter::new(value?)))
            }
            SetupParameterType::Path => {
                let value_length = u8::try_from(read_variable_integer_from_buffer(buf)?)?;

                let value = String::from_utf8(buf.to_vec());

                Ok(SetupParameter::PathParameter(PathParameter {
                    key: SetupParameterType::Path,
                    value_length,
                    value: value?,
                }))
            }
        }
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        todo!()
    }
}

pub(crate) struct RoleParameter {
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

#[derive(Debug, Clone, Copy, IntoPrimitive, TryFromPrimitive)]
#[repr(u8)]
pub enum RoleCase {
    Injection = 0x01,
    Delivery = 0x02,
    Both = 0x03,
}

pub(crate) struct PathParameter {
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

#[derive(Debug, Clone, Copy, IntoPrimitive, TryFromPrimitive)]
#[repr(u8)]
pub(crate) enum SetupParameterType {
    Role = 0x00,
    Path = 0x01,
}
