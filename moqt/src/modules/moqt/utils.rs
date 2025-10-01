use std::any::TypeId;

use anyhow::bail;
use bytes::BytesMut;

use crate::modules::moqt::{
    enums::ReceiveEvent,
    messages::{
        control_message_type::ControlMessageType,
        control_messages::{
            client_setup::ClientSetup, publish_namespace::PublishNamespace,
            publish_namespace_error::PublishNamespaceError,
            publish_namespace_ok::PublishNamespaceOk, server_setup::ServerSetup,
            subscribe_namespace::SubscribeNamespace, subscribe_namespace_ok::SubscribeNamespaceOk,
            util::get_message_type,
        },
        moqt_message::MOQTMessage,
        moqt_message_error::MOQTMessageError, variable_integer::write_variable_integer,
    },
};

pub(crate) async fn start_receive<T: MOQTMessage>(
    mut subscriber: tokio::sync::broadcast::Receiver<ReceiveEvent>,
) -> anyhow::Result<T> {
    let t_message_type = get_message_type_from_t::<T>()?;
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
                if message_type != t_message_type {
                    tracing::info!("Message unmatches. expect: {}, actual: {}", t_message_type as u8, message_type as u8);
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

fn get_message_type_from_t<T: MOQTMessage>() -> anyhow::Result<ControlMessageType> {
    let type_id = TypeId::of::<T>();
    if type_id == TypeId::of::<ClientSetup>() {
        Ok(ControlMessageType::ClientSetup)
    } else if type_id == TypeId::of::<ServerSetup>() {
        Ok(ControlMessageType::ServerSetup)
    } else if type_id == TypeId::of::<PublishNamespace>() {
        Ok(ControlMessageType::PublishNamespace)
    } else if type_id == TypeId::of::<PublishNamespaceOk>() {
        Ok(ControlMessageType::PublishNamespaceOk)
    } else if type_id == TypeId::of::<PublishNamespaceError>() {
        Ok(ControlMessageType::PublishNamespaceError)
    } else if type_id == TypeId::of::<SubscribeNamespace>() {
        Ok(ControlMessageType::SubscribeNamespace)
    } else if type_id == TypeId::of::<SubscribeNamespaceOk>() {
        Ok(ControlMessageType::SubscribeNamespaceOk)
    } else if type_id == TypeId::of::<PublishNamespaceError>() {
        Ok(ControlMessageType::PublishNamespaceError)
    } else {
        bail!("unknown message type.: {:?}", type_id)
    }
}

pub(crate) fn add_message_type<T: MOQTMessage>(payload: BytesMut) -> BytesMut {
    let mut buffer = BytesMut::new();
    let enum_value = get_message_type_from_t::<T>().unwrap() as u8;
    // Message Type
    buffer.extend(write_variable_integer(enum_value as u64));
    buffer.unsplit(payload);
    buffer
}