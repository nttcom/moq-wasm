use bytes::BytesMut;

use crate::modules::moqt::messages::{
    variable_bytes::write_variable_bytes,
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

pub enum VariantType {
    Odd(Vec<u8>),
    Even(u64),
}

pub struct KeyValuePair {
    pub key: u64,
    pub value: VariantType,
}

impl KeyValuePair {
    pub(crate) fn depacketize(bytes: &mut BytesMut) -> anyhow::Result<Self> {
        let key = read_variable_integer_from_buffer(bytes)?;
        if key % 2 == 0 {
            let value = read_variable_integer_from_buffer(bytes)?;
            Ok(Self {
                key,
                value: VariantType::Even(value),
            })
        } else {
            let value_length = read_variable_integer_from_buffer(bytes)?;
            let value = bytes.split_to(value_length as usize).to_vec();
            Ok(Self {
                key,
                value: VariantType::Odd(value),
            })
        }
    }

    pub(crate) fn packetize(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.extend(write_variable_integer(self.key));
        match &self.value {
            VariantType::Odd(value) => {
                buf.extend(write_variable_bytes(value));
            }
            VariantType::Even(value) => {
                buf.extend(write_variable_integer(*value));
            }
        }
        buf
    }
}
