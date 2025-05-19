use bytes::BytesMut;
use moqt_core::{
    control_message_type::ControlMessageType,
    messages::{
        control_messages::{
            announce::Announce, announce_ok::AnnounceOk, subscribe::Subscribe,
            subscribe_announces::SubscribeAnnounces, subscribe_announces_ok::SubscribeAnnouncesOk,
            subscribe_error::SubscribeError, subscribe_ok::SubscribeOk, unsubscribe::Unsubscribe,
        },
        moqt_payload::MOQTPayload,
    },
    variable_integer::write_variable_integer,
};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc::Receiver};
use tracing::{self};
use wtransport::SendStream;

pub(crate) async fn send_control_stream(
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
            .downcast_ref::<SubscribeAnnounces>()
            .is_some()
        {
            message_buf.extend(write_variable_integer(u8::from(
                ControlMessageType::SubscribeAnnounces,
            ) as u64));
            tracing::info!(
                "Relayed Message Type: {:?}",
                ControlMessageType::SubscribeAnnounces
            );
        } else if message
            .as_any()
            .downcast_ref::<SubscribeAnnouncesOk>()
            .is_some()
        {
            message_buf.extend(write_variable_integer(u8::from(
                ControlMessageType::SubscribeAnnouncesOk,
            ) as u64));
            tracing::info!(
                "Relayed Message Type: {:?}",
                ControlMessageType::SubscribeAnnouncesOk
            );
        } else if message.as_any().downcast_ref::<Unsubscribe>().is_some() {
            message_buf.extend(write_variable_integer(
                u8::from(ControlMessageType::UnSubscribe) as u64,
            ));
            tracing::info!(
                "Relayed Message Type: {:?}",
                ControlMessageType::UnSubscribe
            )
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
