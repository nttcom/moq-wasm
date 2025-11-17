use bytes::BytesMut;

use crate::modules::moqt::messages::{
    control_message_type::ControlMessageType, control_messages::util::get_message_type,
    moqt_message::MOQTMessage,
    variable_integer::write_variable_integer,
};

pub(crate) fn add_message_type(message_type: ControlMessageType, payload: BytesMut) -> BytesMut {
    let mut buffer = BytesMut::new();
    // Message Type
    buffer.extend(write_variable_integer(message_type as u64));
    buffer.unsplit(payload);
    buffer
}

pub(crate) fn create_full_message<T: MOQTMessage>(
    message_type: ControlMessageType,
    payload: T,
) -> BytesMut {
    let bytes = payload.packetize();

    let mut buffer = BytesMut::new();
    // Message Type
    buffer.extend(write_variable_integer(message_type as u64));
    buffer.unsplit(bytes);
    buffer
}
