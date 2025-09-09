use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::AtomicU64;

use crate::modules::moqt::constants;
use crate::modules::moqt::messages::control_messages::setup_parameters::MaxSubscribeID;
use crate::modules::moqt::messages::control_messages::setup_parameters::SetupParameter;
use crate::modules::moqt::moqt_connection_message_controller::MOQTConnectionMessageController;
use crate::modules::moqt::moqt_control_receiver::MOQTControlReceiver;
use crate::modules::moqt::moqt_control_sender::MOQTControlSender;
use crate::modules::moqt::moqt_enums::ReceiveEvent;
use crate::modules::transport::transport_connection::TransportConnection;

pub struct MOQTConnection {
    transport_connection: Box<dyn TransportConnection>,
    message_controller: MOQTConnectionMessageController,
    sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    receive_stream: MOQTControlReceiver,
    send_stream: Weak<tokio::sync::Mutex<MOQTControlSender>>,
    pub(crate) request_id: AtomicU64,
}

impl MOQTConnection {
    pub(crate) async fn new(
        is_client: bool,
        transport_connection: Box<dyn TransportConnection>,
        send_stream: MOQTControlSender,
        receive_stream: MOQTControlReceiver,
        sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    ) -> anyhow::Result<Arc<Self>> {
        let shared_send_stream = Arc::new(tokio::sync::Mutex::new(send_stream));
        let mut message_controller =
            MOQTConnectionMessageController::new(shared_send_stream.clone(), sender.clone());
        let request_id = if is_client {
            Self::setup_for_client(&mut message_controller).await?;
            AtomicU64::new(0)
        } else {
            Self::setup_for_server(&mut message_controller).await?;
            AtomicU64::new(1)
        };

        Ok(Arc::new(Self {
            transport_connection,
            message_controller,
            sender,
            receive_stream,
            send_stream: Arc::downgrade(&shared_send_stream),
            request_id,
        }))
    }

    async fn setup_for_client(
        message_controller: &mut MOQTConnectionMessageController,
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
