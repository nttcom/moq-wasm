use std::sync::Arc;

use anyhow::bail;
use bytes::BytesMut;

use crate::modules::moqt::constants;
use crate::modules::moqt::messages::control_messages::client_setup::ClientSetup;
use crate::modules::moqt::messages::control_messages::server_setup::ServerSetup;
use crate::modules::moqt::messages::control_messages::setup_parameters::MaxSubscribeID;
use crate::modules::moqt::messages::control_messages::setup_parameters::SetupParameter;
use crate::modules::moqt::messages::moqt_message::MOQTMessage;
use crate::modules::moqt::messages::moqt_message_error::MOQTMessageError;
use crate::modules::moqt::moqt_control_sender::MOQTControlSender;
use crate::modules::moqt::moqt_enums::ReceiveEvent;

pub(crate) struct MOQTConnectionMessageController {
    send_stream: Arc<tokio::sync::Mutex<MOQTControlSender>>,
    sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
}

impl MOQTConnectionMessageController {
    pub fn new(
        send_stream: Arc<tokio::sync::Mutex<MOQTControlSender>>,
        sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    ) -> Self {
        Self {
            send_stream,
            sender,
        }
    }

    pub(crate) async fn client_setup(
        &mut self,
        supported_versions: Vec<u32>,
        setup_parameters: Vec<SetupParameter>,
    ) -> anyhow::Result<()> {
        let bytes = ClientSetup::new(supported_versions, setup_parameters).packetize();
        self.send_stream
            .lock()
            .await
            .send(&bytes)
            .await
            .inspect_err(|e| tracing::error!("failed to send. :{}", e.to_string()))?;
        tracing::info!("Sent client setup.");

        self.start_receive::<ServerSetup>().await
    }

    pub(crate) async fn server_setup(&self) -> anyhow::Result<()> {
        tracing::info!("Waiting for server setup.");
        self.start_receive::<ClientSetup>().await?;
        tracing::info!("Received client setup.");

        let max_id = MaxSubscribeID::new(1000);
        let bytes = ServerSetup::new(
            constants::MOQ_TRANSPORT_VERSION,
            vec![SetupParameter::MaxSubscribeID(max_id)],
        )
        .packetize();
        self.send_stream
            .lock()
            .await
            .send(&bytes)
            .await
            .inspect_err(|e| tracing::error!("failed to send. :{}", e.to_string()))
            .inspect(|_| tracing::debug!("ServerSetup has been sent."))
    }

    async fn start_receive<T: MOQTMessage>(&self) -> anyhow::Result<()> {
        let mut subscriber = self.sender.subscribe();
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
                        Ok(_) => return Ok(()),
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
}
