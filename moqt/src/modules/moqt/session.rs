use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::AtomicU64;

use crate::modules::moqt::constants;
use crate::modules::moqt::control_receiver::ControlReceiver;
use crate::modules::moqt::control_sender::ControlSender;
use crate::modules::moqt::messages::control_messages::setup_parameters::MaxSubscribeID;
use crate::modules::moqt::messages::control_messages::setup_parameters::SetupParameter;
use crate::modules::moqt::moqt_enums::ReceiveEvent;
use crate::modules::moqt::protocol::TransportProtocol;
use crate::modules::moqt::session_message_controller::SessionMessageController;

pub struct Session<T: TransportProtocol> {
    transport_connection: T::Connection,
    message_controller: SessionMessageController<T>,
    event_sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    receive_stream: ControlReceiver,
    send_stream: Weak<tokio::sync::Mutex<ControlSender<T>>>,
    pub(crate) request_id: AtomicU64,
}

impl<T: TransportProtocol> Session<T> {
    pub(crate) async fn for_client(
        transport_connection: T::Connection,
        send_stream: ControlSender<T>,
        receive_stream: ControlReceiver,
        event_sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    ) -> anyhow::Result<Arc<Self>> {
        let shared_send_stream = Arc::new(tokio::sync::Mutex::new(send_stream));
        let mut message_controller = SessionMessageController {
            send_stream: shared_send_stream.clone(),
            event_sender: event_sender.clone(),
        };
        Self::setup_for_client(&mut message_controller).await?;

        Ok(Arc::new(Self {
            transport_connection,
            message_controller,
            event_sender,
            receive_stream,
            send_stream: Arc::downgrade(&shared_send_stream),
            request_id: AtomicU64::new(0),
        }))
    }

    pub(crate) async fn for_server(
        transport_connection: T::Connection,
        send_stream: ControlSender<T>,
        receive_stream: ControlReceiver,
        event_sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    ) -> anyhow::Result<Arc<Self>> {
        let shared_send_stream = Arc::new(tokio::sync::Mutex::new(send_stream));
        let mut message_controller = SessionMessageController {
            send_stream: shared_send_stream.clone(),
            event_sender: event_sender.clone(),
        };
        Self::setup_for_server(&mut message_controller).await?;

        Ok(Arc::new(Self {
            transport_connection,
            message_controller,
            event_sender,
            receive_stream,
            send_stream: Arc::downgrade(&shared_send_stream),
            request_id: AtomicU64::new(1),
        }))
    }

    async fn setup_for_client(
        message_controller: &mut SessionMessageController<T>,
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
        message_controller: &SessionMessageController<T>,
    ) -> anyhow::Result<()> {
        message_controller.server_setup().await
    }

    pub async fn create_publisher() {}

    pub async fn create_subscriber() {}
}

impl<T: TransportProtocol> Drop for Session<T> {
    fn drop(&mut self) {
        // send goaway
    }
}
