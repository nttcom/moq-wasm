use bytes::BytesMut;
use moqt_core::{
    control_message_type::ControlMessageType,
    messages::{
        control_messages::{
            announce::Announce, announce_ok::AnnounceOk, subscribe::Subscribe,
            subscribe_error::SubscribeError, subscribe_namespace::SubscribeNamespace,
            subscribe_namespace_ok::SubscribeNamespaceOk, subscribe_ok::SubscribeOk,
        },
        moqt_payload::MOQTPayload,
    },
    variable_integer::write_variable_integer,
};
use std::sync::Arc;
use tokio::sync::{mpsc::Receiver, Mutex};
use tracing::{self};
use wtransport::SendStream;

pub(crate) async fn forward_control_message(
    send_stream: Arc<Mutex<SendStream>>,
    mut message_rx: Receiver<Arc<Box<dyn MOQTPayload>>>,
) {
    while let Some(message) = message_rx.recv().await {
        let mut write_buf = BytesMut::new();
        message.packetize(&mut write_buf);
        let mut message_buf = BytesMut::with_capacity(write_buf.len() + 8);

        if message.as_any().downcast_ref::<Subscribe>().is_some() {
            message_buf.extend(write_variable_integer(
                u8::from(ControlMessageType::Subscribe) as u64,
            ));
            tracing::info!("Relayed Message Type: {:?}", ControlMessageType::Subscribe);
        } else if message.as_any().downcast_ref::<SubscribeOk>().is_some() {
            message_buf.extend(write_variable_integer(
                u8::from(ControlMessageType::SubscribeOk) as u64,
            ));
            tracing::info!(
                "Relayed Message Type: {:?}",
                ControlMessageType::SubscribeOk
            );
        } else if message.as_any().downcast_ref::<SubscribeError>().is_some() {
            message_buf.extend(write_variable_integer(
                u8::from(ControlMessageType::SubscribeError) as u64,
            ));
            tracing::info!(
                "Relayed Message Type: {:?}",
                ControlMessageType::SubscribeError
            );
        } else if message.as_any().downcast_ref::<Announce>().is_some() {
            message_buf.extend(write_variable_integer(
                u8::from(ControlMessageType::Announce) as u64,
            ));
            tracing::info!("Relayed Message Type: {:?}", ControlMessageType::Announce);
        } else if message.as_any().downcast_ref::<AnnounceOk>().is_some() {
            message_buf.extend(write_variable_integer(
                u8::from(ControlMessageType::AnnounceOk) as u64,
            ));
            tracing::info!("Relayed Message Type: {:?}", ControlMessageType::AnnounceOk);
        } else if message
            .as_any()
            .downcast_ref::<SubscribeNamespace>()
            .is_some()
        {
            message_buf.extend(write_variable_integer(u8::from(
                ControlMessageType::SubscribeNamespace,
            ) as u64));
            tracing::info!(
                "Relayed Message Type: {:?}",
                ControlMessageType::SubscribeNamespace
            );
        } else if message
            .as_any()
            .downcast_ref::<SubscribeNamespaceOk>()
            .is_some()
        {
            message_buf.extend(write_variable_integer(u8::from(
                ControlMessageType::SubscribeNamespaceOk,
            ) as u64));
            tracing::info!(
                "Relayed Message Type: {:?}",
                ControlMessageType::SubscribeNamespaceOk
            );
        } else {
            tracing::warn!("Unsupported message type for bi-directional stream");
            continue;
        }
        message_buf.extend(write_variable_integer(write_buf.len() as u64));
        message_buf.extend(write_buf);

        let mut shared_send_stream = send_stream.lock().await;
        if let Err(e) = shared_send_stream.write_all(&message_buf).await {
            tracing::warn!("Failed to write to stream: {:?}", e);
            break;
        }

        tracing::info!("Control message is forwarded.");
        // tracing::debug!("forwarded message: {:?}", message_buf.to_vec());
    }
}
