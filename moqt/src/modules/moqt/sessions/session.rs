use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use crate::Publisher;
use crate::Subscriber;
use crate::modules::moqt::constants;
use crate::modules::moqt::control_receiver::ControlReceiver;
use crate::modules::moqt::control_sender::ControlSender;
use crate::modules::moqt::enums::ReceiveEvent;
use crate::modules::moqt::messages::control_messages::setup_parameters::MaxSubscribeID;
use crate::modules::moqt::messages::control_messages::setup_parameters::SetupParameter;
use crate::modules::moqt::protocol::TransportProtocol;
use crate::modules::moqt::sessions::session_message_controller::SessionMessageController;
use crate::modules::transport::transport_connection::TransportConnection;

pub struct Session<T: TransportProtocol> {
    pub id: usize,
    _transport_connection: T::Connection,
    _message_controller: SessionMessageController<T>,
    event_sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    _receive_stream: ControlReceiver,
    send_stream: Arc<tokio::sync::Mutex<ControlSender<T>>>,
    request_id: AtomicU64,
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
            id: transport_connection.id(),
            _transport_connection: transport_connection,
            _message_controller: message_controller,
            event_sender,
            _receive_stream: receive_stream,
            send_stream: shared_send_stream,
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
        let message_controller = SessionMessageController {
            send_stream: shared_send_stream.clone(),
            event_sender: event_sender.clone(),
        };
        Self::setup_for_server(&message_controller).await?;

        Ok(Arc::new(Self {
            id: transport_connection.id(),
            _transport_connection: transport_connection,
            _message_controller: message_controller,
            event_sender,
            _receive_stream: receive_stream,
            send_stream: shared_send_stream,
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

    pub(crate) fn get_request_id(self: &Arc<Self>) -> u64 {
        let id = self.request_id.load(Ordering::SeqCst);
        self.request_id.fetch_add(2, Ordering::SeqCst);
        id
    }

    pub fn create_publisher(self: &Arc<Self>) -> Publisher<T> {
        Publisher::<T> {
            session: self.clone(),
            shared_send_stream: self.send_stream.clone(),
            event_sender: self.event_sender.clone(),
        }
    }

    pub fn create_subscriber(self: &Arc<Self>) -> Subscriber<T> {
        Subscriber::<T> {
            session: self.clone(),
            shared_send_stream: self.send_stream.clone(),
            event_sender: self.event_sender.clone(),
        }
    }
}

impl<T: TransportProtocol> Drop for Session<T> {
    fn drop(&mut self) {
        // send goaway
    }
}
