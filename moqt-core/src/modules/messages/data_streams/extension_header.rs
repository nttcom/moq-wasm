use crate::{
    messages::data_streams::DataStreams,
    variable_bytes::read_fixed_length_bytes,
    variable_integer::{read_variable_integer, write_variable_integer},
};
use anyhow::{bail, Context, Result};
use bytes::BytesMut;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ExtensionHeader {
    header_type: u64,
    value: ExtensionHeaderValue,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum ExtensionHeaderValue {
    EvenTypeValue(Value),
    OddTypeValue(ValueWithLength),
}

impl ExtensionHeader {
    pub fn new(header_type: u64, value: ExtensionHeaderValue) -> Result<Self> {
        if header_type % 2 == 0 && matches!(value, ExtensionHeaderValue::OddTypeValue(_)) {
            bail!("Mismatched value type: expected even, but got odd");
        }

        if header_type % 2 != 0 && matches!(value, ExtensionHeaderValue::EvenTypeValue(_)) {
            bail!("Mismatched value type: expected odd, but got even");
        }

        Ok(ExtensionHeader { header_type, value })
    }

    pub fn byte_length(&self) -> usize {
        let mut len = write_variable_integer(self.header_type).len();
        match &self.value {
            ExtensionHeaderValue::EvenTypeValue(value) => len += value.byte_length(),
            ExtensionHeaderValue::OddTypeValue(value_with_length) => {
                len += value_with_length.byte_length()
            }
        }
        len
    }
}

impl DataStreams for ExtensionHeader {
    fn depacketize(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<Self>
    where
        Self: Sized,
    {
        let header_type = read_variable_integer(read_cur).context("header type")?;
        if header_type % 2 == 0 {
            let value = ExtensionHeaderValue::EvenTypeValue(Value::depacketize(read_cur)?);
            Ok(ExtensionHeader { header_type, value })
        } else {
            let value = ExtensionHeaderValue::OddTypeValue(ValueWithLength::depacketize(read_cur)?);
            Ok(ExtensionHeader { header_type, value })
        }
    }

    fn packetize(&self, buf: &mut BytesMut) {
        buf.extend(write_variable_integer(self.header_type));
        match &self.value {
            ExtensionHeaderValue::EvenTypeValue(value) => value.packetize(buf),
            ExtensionHeaderValue::OddTypeValue(value_with_length) => {
                value_with_length.packetize(buf)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Value {
    header_value: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ValueWithLength {
    header_length: u64,
    header_value: Vec<u8>,
}

impl Value {
    pub fn new(header_value: u64) -> Self {
        Value { header_value }
    }

    pub fn byte_length(&self) -> usize {
        write_variable_integer(self.header_value).len()
    }
}

impl DataStreams for Value {
    fn depacketize(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<Self>
    where
        Self: Sized,
    {
        let header_value = read_variable_integer(read_cur).context("header length")?;

        Ok(Value { header_value })
    }

    fn packetize(&self, buf: &mut BytesMut) {
        buf.extend(write_variable_integer(self.header_value));
    }
}

impl ValueWithLength {
    pub fn new(header_value: Vec<u8>) -> Self {
        ValueWithLength {
            header_length: header_value.len() as u64,
            header_value,
        }
    }

    pub fn byte_length(&self) -> usize {
        let mut len = write_variable_integer(self.header_length).len();
        len += self.header_value.len();
        len
    }
}

impl DataStreams for ValueWithLength {
    fn depacketize(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<Self>
    where
        Self: Sized,
    {
        let header_length = read_variable_integer(read_cur).context("header length")?;
        let header_value = if header_length > 0 {
            read_fixed_length_bytes(read_cur, header_length as usize).context("header value")?
        } else {
            vec![]
        };

        Ok(ValueWithLength {
            header_length,
            header_value,
        })
    }

    fn packetize(&self, buf: &mut BytesMut) {
        buf.extend(write_variable_integer(self.header_length));
        buf.extend(&self.header_value);
    }
}

#[cfg(test)]
mod failure {
    use super::ValueWithLength;
    use crate::messages::data_streams::extension_header::{
        ExtensionHeader, ExtensionHeaderValue, Value,
    };

    #[test]
    fn new_odd_value_with_even_type() {
        let even_header_type = 0;
        let odd_type_value = ExtensionHeaderValue::OddTypeValue(ValueWithLength::new(vec![0]));
        let extension_header = ExtensionHeader::new(even_header_type, odd_type_value);

        assert!(extension_header.is_err());
    }

    #[test]
    fn new_even_value_with_odd_type() {
        let odd_header_type = 1;
        let even_type_value = ExtensionHeaderValue::EvenTypeValue(Value::new(0));
        let extension_header = ExtensionHeader::new(odd_header_type, even_type_value);

        assert!(extension_header.is_err());
    }
}
