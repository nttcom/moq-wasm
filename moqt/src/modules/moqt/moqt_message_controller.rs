use std::{future::Future, sync::Arc, time::Duration};

use anyhow::bail;
use bytes::BytesMut;

use crate::modules::moqt::{
    constants,
    messages::{
        control_messages::{
            client_setup::ClientSetup,
            server_setup::ServerSetup,
            setup_parameters::{MaxSubscribeID, SetupParameter},
        },
        moqt_payload::MOQTPayload,
    },
    moqt_bi_stream::MOQTBiStream,
};

#[derive(Clone)]
enum ReceiveMessage {
    OnMessage(BytesMut),
    OnError,
}

pub(crate) struct MOQTMessageController {
    join_handle: tokio::task::JoinHandle<()>,
    stream: Arc<tokio::sync::Mutex<dyn MOQTBiStream>>,
    sender: tokio::sync::broadcast::Sender<ReceiveMessage>,
}

impl Drop for MOQTMessageController {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

impl MOQTMessageController {
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
                        }
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
        self.stream.lock().await.send(&bytes).await?;

        let closure = self.create_closure::<ServerSetup>();
        self.run_with_timeout(20, closure).await
    }

    pub(crate) async fn server_setup(&self) -> anyhow::Result<()> {
        let closure = self.create_closure::<ClientSetup>();
        self.run_with_timeout(20, closure).await?;
        let mut bytes = BytesMut::new();
        let max_id = MaxSubscribeID::new(1000);
        ServerSetup::new(
            constants::MOQ_TRANSPORT_VERSION,
            vec![SetupParameter::MaxSubscribeID(max_id)],
        )
        .packetize(&mut bytes);
        self.stream.lock().await.send(&bytes).await
    }

    fn create_closure<T: MOQTPayload>(&self) -> impl Future<Output = anyhow::Result<()>> {
        let mut subscriber = self.sender.subscribe();
        async move {
            loop {
                let receive_message: Result<ReceiveMessage, _> = subscriber.recv().await;
                if let Err(e) = receive_message {
                    bail!("failed to receive. {:?}", e.to_string());
                }
                match receive_message.unwrap() {
                    ReceiveMessage::OnMessage(mut bytes_mut) => {
                        match T::depacketize(&mut bytes_mut) {
                            Ok(_) => return Ok(()),
                            Err(_) => {
                                tracing::info!("unmatch message.");
                                continue;
                            }
                        };
                    }
                    ReceiveMessage::OnError => bail!("failed to receive."),
                }
            }
        }
    }

    async fn run_with_timeout<T: Future<Output = anyhow::Result<()>>>(
        &self,
        timeout_sec: u64,
        future: T,
    ) -> anyhow::Result<()> {
        match tokio::time::timeout(Duration::from_secs(timeout_sec), future).await {
            Ok(_) => Ok(()),
            Err(_) => bail!("timeout"),
        }
    }
}
