use std::any::TypeId;

use anyhow::bail;
use bytes::BytesMut;

use crate::modules::moqt::{
    enums::ReceiveEvent,
    messages::{
        control_message_type::ControlMessageType,
        control_messages::{
            client_setup::ClientSetup, namespace_ok::NamespaceOk,
            publish_namespace::PublishNamespace, request_error::RequestError,
            server_setup::ServerSetup, util::get_message_type,
        },
        moqt_message::MOQTMessage,
        moqt_message_error::MOQTMessageError,
        variable_integer::write_variable_integer,
    },
};

pub(crate) async fn start_receive<T: MOQTMessage>(
    expect_message_type: ControlMessageType,
    mut subscriber: tokio::sync::broadcast::Receiver<ReceiveEvent>,
) -> anyhow::Result<T> {
    loop {
        let receive_message = subscriber.recv().await;
        if let Err(e) = receive_message {
            tracing::info!("failed to receive. {:?}", e.to_string());
            bail!("failed to receive. {:?}", e.to_string());
        }
        tracing::info!("Message has been received.");
        match receive_message.unwrap() {
            ReceiveEvent::Message(data) => {
                let mut bytes_mut = BytesMut::from(data.as_slice());
                let message_type = get_message_type(&mut bytes_mut)?;
                if message_type != expect_message_type {
                    tracing::info!(
                        "Message unmatches. expect: {}, actual: {}",
                        expect_message_type as u8,
                        message_type as u8
                    );
                    continue;
                }
                match T::depacketize(&mut bytes_mut) {
                    Ok(t) => return Ok(t),
                    Err(MOQTMessageError::MessageUnmatches) => {
                        tracing::info!("Message unmatches.");
                        continue;
                    }
                    Err(MOQTMessageError::ProtocolViolation) => {
                        tracing::error!("Protocol violation.");
                        bail!("Protocol violation.");
                    }
                };
            }
            ReceiveEvent::Error() => bail!("failed to receive."),
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
