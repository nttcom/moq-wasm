use anyhow::bail;
use bytes::BytesMut;

use crate::modules::moqt::{
    enums::ReceiveEvent,
    messages::{moqt_message::MOQTMessage, moqt_message_error::MOQTMessageError},
};

pub(crate) async fn start_receive<T: MOQTMessage>(
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
