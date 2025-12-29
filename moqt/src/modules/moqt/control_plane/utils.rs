use bytes::BytesMut;

use crate::modules::{
    extensions::buf_put_ext::BufPutExt,
    moqt::control_plane::messages::control_message_type::ControlMessageType,
};

pub(crate) fn add_message_type(message_type: ControlMessageType, payload: BytesMut) -> BytesMut {
    let mut buffer = BytesMut::new();
    // Message Type
    buffer.put_varint(message_type as u64);
    buffer.unsplit(payload);
    buffer
}

pub(crate) fn create_full_message(message_type: ControlMessageType, payload: BytesMut) -> BytesMut {
    let mut buffer = BytesMut::new();
    // Message Type
    buffer.put_varint(message_type as u64);
    // buffer.put_varint(payload.len() as u64);
    buffer.unsplit(payload);
    buffer
}
