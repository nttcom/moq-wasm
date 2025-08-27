use std::{result, sync::Arc};

use bytes::BytesMut;

use crate::modules::session_handlers::{
    messages::{
        control_messages::{client_setup::ClientSetup, setup_parameters::SetupParameter},
        moqt_payload::MOQTPayload,
    },
    moqt_bi_stream::MOQTBiStream,
};

#[derive(Clone)]
enum ReceiveMessage {
    OnMessage(BytesMut),
    OnError(String),
}

pub(crate) struct MessageJoinHandleManager {
    join_handle: tokio::task::JoinHandle<()>,
    stream: Arc<dyn MOQTBiStream>,
    sender: tokio::sync::broadcast::Sender<ReceiveMessage>,
}

impl Drop for MessageJoinHandleManager {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

impl MessageJoinHandleManager {
    pub fn new(stream: Arc<dyn MOQTBiStream>) -> Self {
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
        stream: Arc<dyn MOQTBiStream>,
        sender: tokio::sync::broadcast::Sender<ReceiveMessage>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Messsage Receiver")
            .spawn(async move {
                loop {
                    let buffer = stream.receive().await;
                    let result = match buffer {
                        Ok(buffer) => {
                            let receive_message = ReceiveMessage::OnMessage(buffer);
                            sender.send(receive_message)
                        },
                        Err(e) => {
                            let receive_message = ReceiveMessage::OnError(e.to_string());
                            sender.send(receive_message)
                        },
                    };
                    match result {
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
        _ = self.stream.send(&bytes).await?;
        loop {
            subscriber.recv().await?;
        }
        Ok(())
    }

    pub(crate) fn server_setup(&self) {}
}
