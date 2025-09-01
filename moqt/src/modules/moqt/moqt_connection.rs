use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::AtomicU64;

use crate::modules::moqt::constants;
use crate::modules::moqt::messages::control_messages::setup_parameters::MaxSubscribeID;
use crate::modules::moqt::messages::control_messages::setup_parameters::SetupParameter;
use crate::modules::moqt::moqt_bi_stream::MOQTBiStream;
use crate::modules::moqt::moqt_bi_stream::ReceiveEvent;
use crate::modules::moqt::moqt_connection_message_controller::MOQTConnectionMessageController;
use crate::modules::transport::transport_connection::TransportConnection;

pub(crate) struct MOQTConnection {
    transport_connection: Box<dyn TransportConnection>,
    message_controller: MOQTConnectionMessageController,
    sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    moqt_bi_stream: Weak<MOQTBiStream>,
    pub(crate) request_id: AtomicU64,
}

impl MOQTConnection {
    pub(crate) async fn new(
        is_client: bool,
        transport_connection: Box<dyn TransportConnection>,
        stream: Arc<MOQTBiStream>,
        sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    ) -> anyhow::Result<Arc<Self>> {
        let message_controller =
            MOQTConnectionMessageController::new(stream.clone(), sender.clone());
        let request_id = if is_client {
            Self::setup_for_client(&message_controller).await?;
            AtomicU64::new(0)
        } else {
            Self::setup_for_server(&message_controller).await?;
            AtomicU64::new(1)
        };

        Ok(Arc::new(Self {
            transport_connection,
            message_controller,
            sender,
            moqt_bi_stream: Arc::downgrade(&stream),
            request_id,
        }))
    }

    async fn setup_for_client(
        message_controller: &MOQTConnectionMessageController,
    ) -> anyhow::Result<()> {
        let max_id = MaxSubscribeID::new(1000);
        message_controller
            .client_setup(
                vec![constants::MOQ_TRANSPORT_VERSION],
                vec![SetupParameter::MaxSubscribeID(max_id)],
            )
            .await
    }

    async fn setup_for_server(
        message_controller: &MOQTConnectionMessageController,
    ) -> anyhow::Result<()> {
        message_controller.server_setup().await
    }

    pub async fn create_publisher() {}

    pub async fn create_subscriber() {}
}

impl Drop for MOQTConnection {
    fn drop(&mut self) {
        // send goaway
    }
}
