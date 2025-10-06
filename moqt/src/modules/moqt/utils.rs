use std::any::Any;

use anyhow::bail;
use bytes::BytesMut;

use crate::modules::moqt::messages::{
    control_message_type::ControlMessageType, control_messages::util::get_message_type,
    moqt_message::MOQTMessage, moqt_message_error::MOQTMessageError,
    variable_integer::write_variable_integer,
};

pub(crate) fn depacketize<T: MOQTMessage>(
    expect_message_type: ControlMessageType,
    bytes: Vec<u8>,
) -> anyhow::Result<T> {
    let mut bytes_mut = BytesMut::from(bytes.as_slice());
    let message_type = get_message_type(&mut bytes_mut)?;
    if message_type != expect_message_type {
        tracing::info!(
            "Message unmatches. expect: {}, actual: {}",
            expect_message_type as u8,
            message_type as u8
        );
        bail!("Protocol violation.");
    }
    match T::depacketize(&mut bytes_mut) {
        Ok(t) => Ok(t),
        Err(MOQTMessageError::MessageUnmatches) => {
            tracing::info!("Message unmatches.");
            bail!("Message unmatches.");
        }
        Err(MOQTMessageError::ProtocolViolation) => {
            tracing::error!("Protocol violation.");
            bail!("Protocol violation.");
        }
    }
}

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
