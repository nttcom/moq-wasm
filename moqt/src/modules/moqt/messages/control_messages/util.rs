use std::io::Cursor;

use anyhow::bail;
use bytes::{Buf, BytesMut};

use crate::modules::moqt::messages::{
    control_message_type::ControlMessageType,
    moqt_message_error::MOQTMessageError,
    variable_integer::{read_variable_integer, write_variable_integer},
};

pub(crate) fn get_message_type(read_buf: &mut BytesMut) -> anyhow::Result<ControlMessageType> {
    let mut read_cur = Cursor::new(&read_buf[..]);
    // Read the message type
    let message_type = read_variable_integer(&mut read_cur)?;
    match ControlMessageType::try_from(message_type as u8) {
        Ok(v) => {
            read_buf.advance(read_cur.position() as usize);
            Ok(v)
        }
        Err(e) => bail!("Failed to convert message type.: {}", e.number),
    }
}

pub(crate) fn validate_payload_length(read_buf: &mut BytesMut) -> bool {
    let mut read_cur = Cursor::new(&read_buf[..]);
    let payload_length = read_variable_integer(&mut read_cur).unwrap();
    read_buf.advance(read_cur.position() as usize);

    if read_buf.len() != payload_length as usize {
        tracing::error!(
            "Message length unmatches. expect {}, actual {}",
            payload_length,
            read_buf.len()
        );
        false
    } else {
        true
    }
}

pub(crate) fn add_payload_length(payload: BytesMut) -> BytesMut {
    let mut buffer = BytesMut::new();
    // Message Type
    buffer.extend(write_variable_integer(payload.len() as u64));
    buffer.unsplit(payload);
    buffer
}

pub(super) fn u8_to_bool(value: u8) -> Result<bool, MOQTMessageError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => {
            tracing::error!("Invalid value for bool: {}", value);
            Err(MOQTMessageError::ProtocolViolation)
        }
    }
}
