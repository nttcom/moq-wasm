use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use crate::{
    TransportProtocol,
    modules::{
        moqt::{
            constants,
            control_receiver::ControlReceiver,
            control_sender::ControlSender,
            enums::ReceiveEvent,
            messages::{
                control_messages::{
                    client_setup::ClientSetup,
                    server_setup::ServerSetup,
                    setup_parameters::{MaxSubscribeID, SetupParameter},
                },
                moqt_message::MOQTMessage,
            },
            utils::{self, add_message_type},
        },
        transport::transport_connection::TransportConnection,
    },
};

pub(crate) struct InnerSession<T: TransportProtocol> {
    pub(crate) id: usize,
    _transport_connection: T::Connection,
    pub(super) event_sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    _receive_stream: ControlReceiver,
    pub(crate) send_stream: Arc<tokio::sync::Mutex<ControlSender<T>>>,
    request_id: AtomicU64,
}

impl<T: TransportProtocol> InnerSession<T> {
    pub(crate) async fn client(
        transport_connection: T::Connection,
        mut send_stream: ControlSender<T>,
        receive_stream: ControlReceiver,
        event_sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    ) -> anyhow::Result<Self> {
        Self::setup_client(&mut send_stream, event_sender.subscribe()).await?;
        let shared_send_stream = Arc::new(tokio::sync::Mutex::new(send_stream));
        Ok(Self {
            id: transport_connection.id(),
            _transport_connection: transport_connection,
            event_sender,
            _receive_stream: receive_stream,
            send_stream: shared_send_stream,
            request_id: AtomicU64::new(0),
        })
    }

    pub(crate) async fn server(
        transport_connection: T::Connection,
        mut send_stream: ControlSender<T>,
        receive_stream: ControlReceiver,
        event_sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    ) -> anyhow::Result<Self> {
        Self::setup_server(&mut send_stream, event_sender.subscribe()).await?;

        let shared_send_stream = Arc::new(tokio::sync::Mutex::new(send_stream));
        Ok(Self {
            id: transport_connection.id(),
            _transport_connection: transport_connection,
            event_sender,
            _receive_stream: receive_stream,
            send_stream: shared_send_stream,
            request_id: AtomicU64::new(1),
        })
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

    pub(crate) fn get_request_id(&self) -> u64 {
        let id = self.request_id.load(Ordering::SeqCst);
        self.request_id.fetch_add(2, Ordering::SeqCst);
        id
    }
}

impl<T: TransportProtocol> Drop for InnerSession<T> {
    fn drop(&mut self) {
        // send goaway
    }
}
