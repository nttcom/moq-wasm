use std::{sync::Arc, time::Duration};

use anyhow::bail;
use bytes::BytesMut;
use tokio::time::timeout;

use crate::modules::session_handlers::{
    messages::{
        control_messages::{client_setup::ClientSetup, server_setup::ServerSetup, setup_parameters::SetupParameter},
        moqt_payload::MOQTPayload,
    },
    moqt_bi_stream::MOQTBiStream,
};

#[derive(Clone)]
enum ReceiveMessage {
    OnMessage(BytesMut),
    OnError,
}

pub(crate) struct MessageJoinHandleManager {
    join_handle: tokio::task::JoinHandle<()>,
    stream: Arc<tokio::sync::Mutex<dyn MOQTBiStream>>,
    sender: tokio::sync::broadcast::Sender<ReceiveMessage>,
}

impl Drop for MessageJoinHandleManager {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

impl MessageJoinHandleManager {
    pub fn new(stream: Arc<tokio::sync::Mutex<dyn MOQTBiStream>>) -> Self {
        let (sender, _) = tokio::sync::broadcast::channel::<ReceiveMessage>(1024);
        let _sender = sender.clone();
        let join_handle = Self::create_join_handle(stream.clone(), sender.clone());
        Self {
            join_handle,
            stream,
            sender,
        }
    }

    fn create_join_handle(
        stream: Arc<tokio::sync::Mutex<dyn MOQTBiStream>>,
        sender: tokio::sync::broadcast::Sender<ReceiveMessage>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Messsage Receiver")
            .spawn(async move {
                loop {
                    let buffer = stream.lock().await.receive().await;
                    let message = match buffer {
                        Ok(buffer) => ReceiveMessage::OnMessage(buffer),
                        Err(e) => {
                            tracing::error!("failed to receive. {:?}", e.to_string());
                            ReceiveMessage::OnError
                        },
                    };
                    match sender.send(message) {
                        Ok(_) => tracing::info!("send ok"),
                        Err(e) => tracing::error!("failed to send. {:?}", e.to_string()),
                    }
                }
            })
            .unwrap()
    }

    pub(crate) async fn client_setup(
        &self,
        supported_versions: Vec<u32>,
        setup_parameters: Vec<SetupParameter>,
    ) -> anyhow::Result<()> {
        let mut bytes = BytesMut::new();
        ClientSetup::new(supported_versions, setup_parameters).packetize(&mut bytes);
        let mut subscriber = self.sender.subscribe();
        _ = self.stream.lock().await.send(&bytes).await?;
        timeout(Duration::from_secs(20), async {
            loop {
                let receive_message = subscriber.recv().await;
                if let Err(e) = receive_message {
                    bail!("failed to receive. {:?}", e.to_string());
                }
                match receive_message.unwrap() {
                    ReceiveMessage::OnMessage(mut bytes_mut) => {
                        match ServerSetup::depacketize(&mut bytes_mut) {
                            Ok(_) => return Ok(()),
                            Err(_) => {
                                tracing::info!("unmatch message.");
                                continue;
                            },
                        };
                    },
                    ReceiveMessage::OnError => bail!("failed to receive."),
                }
            }
        }).await?
    }

    pub(crate) fn server_setup(&self) {}
}
