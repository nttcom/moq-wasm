use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::bail;

use crate::{
    modules::moqt::{
        constants,
        controls::{control_receiver::ControlReceiver, control_sender::ControlSender},
        enums::ResponseMessage,
        messages::{
            control_message_type::ControlMessageType,
            control_messages::{
                client_setup::ClientSetup,
                server_setup::ServerSetup,
                setup_parameters::{MaxSubscribeID, SetupParameter},
            },
            moqt_message::MOQTMessage,
        },
        utils::{self, add_message_type},
    }, RequestId, SessionEvent, TransportProtocol
};

pub(crate) struct InnerSession<T: TransportProtocol> {
    _transport_connection: T::Connection,
    pub(crate) send_stream: ControlSender<T>,
    pub(crate) receive_stream: ControlReceiver<T>,
    request_id: AtomicU64,
    pub(crate) event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    pub(crate) sender_map:
        tokio::sync::Mutex<HashMap<RequestId, tokio::sync::oneshot::Sender<ResponseMessage>>>,
}

impl<T: TransportProtocol> InnerSession<T> {
    pub(crate) async fn client(
        transport_connection: T::Connection,
        mut send_stream: ControlSender<T>,
        receive_stream: ControlReceiver<T>,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>
    ) -> anyhow::Result<Self> {
        Self::setup_client(&mut send_stream, &receive_stream).await?;

        Ok(Self {
            _transport_connection: transport_connection,
            send_stream,
            receive_stream,
            request_id: AtomicU64::new(0),
            event_sender,
            sender_map: tokio::sync::Mutex::new(HashMap::new()),
        })
    }

    pub(crate) async fn server(
        transport_connection: T::Connection,
        mut send_stream: ControlSender<T>,
        receive_stream: ControlReceiver<T>,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>
    ) -> anyhow::Result<Self> {
        Self::setup_server(&mut send_stream, &receive_stream).await?;

        Ok(Self {
            _transport_connection: transport_connection,
            send_stream,
            receive_stream,
            request_id: AtomicU64::new(1),
            event_sender,
            sender_map: tokio::sync::Mutex::new(HashMap::new()),
        })
    }

    async fn setup_client(
        send_stream: &mut ControlSender<T>,
        receive_stream: &ControlReceiver<T>,
    ) -> anyhow::Result<()> {
        let max_id = MaxSubscribeID::new(1000);
        let payload = ClientSetup::new(
            vec![constants::MOQ_TRANSPORT_VERSION],
            vec![SetupParameter::MaxSubscribeID(max_id)],
        )
        .packetize();
        let bytes = add_message_type(ControlMessageType::ClientSetup, payload);
        send_stream
            .send(&bytes)
            .await
            .inspect_err(|e| tracing::error!("failed to send. :{}", e.to_string()))?;
        tracing::info!("Sent client setup.");

        let bytes = receive_stream.receive().await?;
        match utils::depacketize::<ServerSetup>(ControlMessageType::ServerSetup, bytes) {
            Ok(_) => {
                tracing::info!("Received server setup.");
                Ok(())
            }
            Err(e) => {
                tracing::error!("Protocol violation. {}", e.to_string());
                bail!("Protocol violation.")
            }
        }
    }

    async fn setup_server(
        send_stream: &mut ControlSender<T>,
        receive_stream: &ControlReceiver<T>,
    ) -> anyhow::Result<()> {
        tracing::info!("Waiting for server setup.");
        let bytes = receive_stream.receive().await?;
        match utils::depacketize::<ClientSetup>(ControlMessageType::ClientSetup, bytes) {
            Ok(_) => {
                tracing::info!("Received client setup.");
            }
            Err(e) => {
                tracing::error!("Protocol violation. {}", e.to_string());
                bail!("Protocol violation.")
            }
        };
        tracing::info!("Received client setup.");

        let max_id = MaxSubscribeID::new(1000);
        let payload = ServerSetup::new(
            constants::MOQ_TRANSPORT_VERSION,
            vec![SetupParameter::MaxSubscribeID(max_id)],
        )
        .packetize();
        let bytes = add_message_type(ControlMessageType::ServerSetup, payload);
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
