use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use anyhow::bail;

use crate::Publisher;
use crate::Subscriber;
use crate::modules::moqt::constants;
use crate::modules::moqt::control_receiver::ControlReceiver;
use crate::modules::moqt::control_sender::ControlSender;
use crate::modules::moqt::enums::ReceiveEvent;
use crate::modules::moqt::enums::SessionEvent;
use crate::modules::moqt::messages::control_messages::client_setup::ClientSetup;
use crate::modules::moqt::messages::control_messages::server_setup::ServerSetup;
use crate::modules::moqt::messages::control_messages::setup_parameters::MaxSubscribeID;
use crate::modules::moqt::messages::control_messages::setup_parameters::SetupParameter;
use crate::modules::moqt::messages::moqt_message::MOQTMessage;
use crate::modules::moqt::protocol::TransportProtocol;
use crate::modules::moqt::sessions::session_message_resolver::SessionMessageResolver;
use crate::modules::moqt::utils;
use crate::modules::moqt::utils::add_message_type;
use crate::modules::transport::transport_connection::TransportConnection;

pub struct Session<T: TransportProtocol> {
    pub id: usize,
    _transport_connection: T::Connection,
    event_sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    _receive_stream: ControlReceiver,
    send_stream: Arc<tokio::sync::Mutex<ControlSender<T>>>,
    request_id: AtomicU64,
}

impl<T: TransportProtocol> Session<T> {
    pub(crate) async fn client(
        transport_connection: T::Connection,
        mut send_stream: ControlSender<T>,
        receive_stream: ControlReceiver,
        event_sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    ) -> anyhow::Result<Arc<Self>> {
        Self::setup_client(&mut send_stream, event_sender.subscribe()).await?;
        let shared_send_stream = Arc::new(tokio::sync::Mutex::new(send_stream));
        Ok(Arc::new(Self {
            id: transport_connection.id(),
            _transport_connection: transport_connection,
            event_sender,
            _receive_stream: receive_stream,
            send_stream: shared_send_stream,
            request_id: AtomicU64::new(0),
        }))
    }

    pub(crate) async fn server(
        transport_connection: T::Connection,
        mut send_stream: ControlSender<T>,
        receive_stream: ControlReceiver,
        event_sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    ) -> anyhow::Result<Arc<Self>> {
        Self::setup_server(&mut send_stream, event_sender.subscribe()).await?;

        let shared_send_stream = Arc::new(tokio::sync::Mutex::new(send_stream));
        Ok(Arc::new(Self {
            id: transport_connection.id(),
            _transport_connection: transport_connection,
            event_sender,
            _receive_stream: receive_stream,
            send_stream: shared_send_stream,
            request_id: AtomicU64::new(1),
        }))
    }

    async fn setup_client(
        send_stream: &mut ControlSender<T>,
        event_receiver: tokio::sync::broadcast::Receiver<ReceiveEvent>,
    ) -> anyhow::Result<()> {
        let max_id = MaxSubscribeID::new(1000);
        let payload = ClientSetup::new(
            vec![constants::MOQ_TRANSPORT_VERSION],
            vec![SetupParameter::MaxSubscribeID(max_id)],
        )
        .packetize();
        let bytes = add_message_type::<ClientSetup>(payload);
        send_stream
            .send(&bytes)
            .await
            .inspect_err(|e| tracing::error!("failed to send. :{}", e.to_string()))?;
        tracing::info!("Sent client setup.");

        utils::start_receive::<ServerSetup>(event_receiver)
            .await
            .map(|_| ())
    }

    async fn setup_server(
        send_stream: &mut ControlSender<T>,
        event_receiver: tokio::sync::broadcast::Receiver<ReceiveEvent>,
    ) -> anyhow::Result<()> {
        tracing::info!("Waiting for server setup.");
        utils::start_receive::<ClientSetup>(event_receiver).await?;
        tracing::info!("Received client setup.");

        let max_id = MaxSubscribeID::new(1000);
        let payload = ServerSetup::new(
            constants::MOQ_TRANSPORT_VERSION,
            vec![SetupParameter::MaxSubscribeID(max_id)],
        )
        .packetize();
        let bytes = add_message_type::<ServerSetup>(payload);
        send_stream
            .send(&bytes)
            .await
            .inspect_err(|e| tracing::error!("failed to send. :{}", e.to_string()))
            .inspect(|_| tracing::debug!("ServerSetup has been sent."))
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

    pub async fn receive(&self) -> anyhow::Result<SessionEvent> {
        let mut receiver = self.event_sender.subscribe();
        let receive_message = receiver.recv().await?;
        match receive_message {
            ReceiveEvent::Message(binary) => SessionMessageResolver::resolve_message(binary),
            ReceiveEvent::Error() => bail!("Error occurred."),
        }
    }
}

impl<T: TransportProtocol> Drop for Session<T> {
    fn drop(&mut self) {
        // send goaway
    }
}
