use std::future::Future;
use std::sync::Arc;

use anyhow::bail;
use bytes::BytesMut;

use crate::modules::moqt::constants;
use crate::modules::moqt::messages::control_message_type::ControlMessageType;
use crate::modules::moqt::messages::control_messages::client_setup::ClientSetup;
use crate::modules::moqt::messages::control_messages::server_setup::ServerSetup;
use crate::modules::moqt::messages::control_messages::setup_parameters::MaxSubscribeID;
use crate::modules::moqt::messages::control_messages::setup_parameters::SetupParameter;
use crate::modules::moqt::messages::control_messages::util::ValidationResult;
use crate::modules::moqt::messages::control_messages::util::add_header;
use crate::modules::moqt::messages::control_messages::util::validate_header;
use crate::modules::moqt::messages::moqt_payload::MOQTPayload;
use crate::modules::moqt::moqt_bi_stream::MOQTBiStream;
use crate::modules::moqt::moqt_bi_stream::ReceiveEvent;

pub(crate) struct MOQTConnectionMessageController {
    stream: Arc<MOQTBiStream>,
    sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
}

impl MOQTConnectionMessageController {
    pub fn new(
        stream: Arc<MOQTBiStream>,
        sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    ) -> Self {
        Self { stream, sender }
    }

    pub(crate) async fn client_setup(
        &self,
        supported_versions: Vec<u32>,
        setup_parameters: Vec<SetupParameter>,
    ) -> anyhow::Result<()> {
        let mut bytes = BytesMut::new();
        ClientSetup::new(supported_versions, setup_parameters).packetize(&mut bytes);
        let message = add_header(ControlMessageType::ClientSetup as u8, bytes);
        self.stream.send(&message).await?;

        self.create_closure::<ServerSetup>(ControlMessageType::ServerSetup as u8)
            .await
    }

    pub(crate) async fn server_setup(&self) -> anyhow::Result<()> {
        self.create_closure::<ClientSetup>(ControlMessageType::ClientSetup as u8)
            .await?;
        let mut bytes = BytesMut::new();
        let max_id = MaxSubscribeID::new(1000);
        ServerSetup::new(
            constants::MOQ_TRANSPORT_VERSION,
            vec![SetupParameter::MaxSubscribeID(max_id)],
        )
        .packetize(&mut bytes);
        let message = add_header(ControlMessageType::ServerSetup as u8, bytes);
        self.stream.send(&message).await
    }

    fn create_closure<T: MOQTPayload>(
        &self,
        message_type: u8,
    ) -> impl Future<Output = anyhow::Result<()>> {
        let mut subscriber = self.sender.subscribe();
        async move {
            loop {
                let receive_message = subscriber.recv().await;
                if let Err(e) = receive_message {
                    bail!("failed to receive. {:?}", e.to_string());
                }
                match receive_message.unwrap() {
                    ReceiveEvent::Message(mut bytes_mut) => {
                        let header_check = validate_header(&mut bytes_mut, message_type);
                        if matches!(header_check, ValidationResult::Fail) {
                            tracing::info!("unmatch message.");
                            continue;
                        }
                        match T::depacketize(&mut bytes_mut) {
                            Ok(_) => return Ok(()),
                            Err(_) => {
                                tracing::info!("depacketize failed.");
                                continue;
                            }
                        };
                    }
                    ReceiveEvent::Error() => bail!("failed to receive."),
                }
            }
        }
    }
}
