use std::io::Cursor;

use bytes::{Buf, BytesMut};

use crate::modules::moqt::messages::{
    control_message_type::ControlMessageType, moqt_message_error::MOQTMessageError, variable_integer::{read_variable_integer, write_variable_integer}
};

pub(crate) fn validate_header( enum_value: u8, read_buf: &mut BytesMut) -> Result<(), MOQTMessageError> {
    let mut read_cur = Cursor::new(&read_buf[..]);
    // Read the message type
    let message_type = match read_variable_integer(&mut read_cur) {
        Ok(v) => v as u8,
        Err(_) => return Err(MOQTMessageError::ProtocolViolation),
    };
    let message_type = match ControlMessageType::try_from(message_type) {
        Ok(m) => m,
        Err(_) => return Err(MOQTMessageError::ProtocolViolation),
    };
    if message_type as u8 != enum_value {
        read_buf.advance(read_cur.position() as usize);
        tracing::warn!("message_type is wrong.");
        return Err(MOQTMessageError::MessageUnmatches);
    }

    let payload_length = read_variable_integer(&mut read_cur).unwrap();
    read_buf.advance(read_cur.position() as usize);

    if read_buf.len() != payload_length as usize {
        tracing::error!("Message length unmatches. expect {}, actual {}", payload_length, read_buf.len());
        return Err(MOQTMessageError::ProtocolViolation);
    }

    Ok(())
}

pub(crate) fn add_header(enum_value: u8, payload: BytesMut) -> BytesMut {
    let mut buffer = BytesMut::new();
    // Message Type
    buffer.extend(write_variable_integer(enum_value as u64));
    // Message Length
    buffer.extend(write_variable_integer(payload.len() as u64));
    buffer.unsplit(payload);
    buffer
}
