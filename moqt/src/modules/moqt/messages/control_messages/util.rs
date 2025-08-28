use std::io::Cursor;

use bytes::{Buf, BytesMut};

use crate::modules::moqt::messages::{
    control_message_type::ControlMessageType,
    variable_integer::{read_variable_integer, write_variable_integer},
};

pub(crate) enum ValidationResult {
    Success,
    Fragment,
    Fail,
}

pub(crate) fn validate_header(read_buf: &mut BytesMut, enum_value: u8) -> ValidationResult {
    let mut read_cur = Cursor::new(&read_buf[..]);
    // Read the message type
    let message_type = match read_variable_integer(&mut read_cur) {
        Ok(v) => v as u8,
        Err(_) => return ValidationResult::Fail,
    };
    let message_type = match ControlMessageType::try_from(message_type) {
        Ok(m) => m,
        Err(_) => return ValidationResult::Fail,
    };
    if message_type as u8 != enum_value {
        read_buf.advance(read_cur.position() as usize);
        tracing::warn!("message_type is wrong.");
        return ValidationResult::Fail;
    }

    let payload_length = read_variable_integer(&mut read_cur).unwrap();
    if payload_length == 0 {
        // The length is insufficient, so do nothing. Do not synchronize with the cursor.
        tracing::error!("fragmented {}", read_buf.len());
        return ValidationResult::Fragment;
    }

    read_buf.advance(read_cur.position() as usize);
    ValidationResult::Success
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
