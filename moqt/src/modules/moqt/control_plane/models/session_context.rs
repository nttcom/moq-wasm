use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::bail;

use crate::{
    ObjectDatagram, SessionEvent, TransportProtocol,
    modules::moqt::control_plane::{
        constants::{self, MOQ_TRANSPORT_VERSION},
        enums::{RequestId, ResponseMessage},
        messages::{
            control_message_type::ControlMessageType,
            control_messages::{
                client_setup::ClientSetup, server_setup::ServerSetup,
                setup_parameters::SetupParameter, util,
            },
        },
        utils::add_message_type,
    },
    modules::moqt::data_plane::streams::stream::{
        stream_receiver::StreamReceiver, stream_sender::StreamSender,
    },
};

#[derive(Debug)]
pub(crate) struct SessionContext<T: TransportProtocol> {
    pub(crate) transport_connection: T::Connection,
    pub(crate) send_stream: StreamSender<T>,
    request_id: AtomicU64,
    pub(crate) event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent<T>>,
    pub(crate) sender_map:
        tokio::sync::Mutex<HashMap<RequestId, tokio::sync::oneshot::Sender<ResponseMessage>>>,
    pub(crate) datagram_sender_map:
        tokio::sync::RwLock<HashMap<u64, tokio::sync::mpsc::UnboundedSender<ObjectDatagram>>>,
}

impl<T: TransportProtocol> SessionContext<T> {
    pub(crate) async fn client(
        transport_connection: T::Connection,
        mut send_stream: StreamSender<T>,
        receive_stream: &mut StreamReceiver<T>,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent<T>>,
    ) -> anyhow::Result<Self> {
        Self::setup_client(&mut send_stream, receive_stream).await?;

        Ok(Self {
            transport_connection,
            send_stream,
            request_id: AtomicU64::new(0),
            event_sender,
            sender_map: tokio::sync::Mutex::new(HashMap::new()),
            datagram_sender_map: tokio::sync::RwLock::new(HashMap::new()),
        })
    }

    pub(crate) async fn server(
        transport_connection: T::Connection,
        mut send_stream: StreamSender<T>,
        receive_stream: &mut StreamReceiver<T>,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent<T>>,
    ) -> anyhow::Result<Self> {
        Self::setup_server(&mut send_stream, receive_stream).await?;

        Ok(Self {
            transport_connection,
            send_stream,
            request_id: AtomicU64::new(1),
            event_sender,
            sender_map: tokio::sync::Mutex::new(HashMap::new()),
            datagram_sender_map: tokio::sync::RwLock::new(HashMap::new()),
        })
    }

    async fn setup_client(
        send_stream: &mut StreamSender<T>,
        receive_stream: &mut StreamReceiver<T>,
    ) -> anyhow::Result<()> {
        let setup_param = SetupParameter {
            path: None,
            max_request_id: 1000,
            authorization_token: vec![],
            max_auth_token_cache_size: None,
            authority: None,
            moq_implementation: Some("MOQ-WASM".to_string()),
        };
        let payload =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_param).encode();
        let bytes = add_message_type(ControlMessageType::ClientSetup, payload);
        send_stream
            .send(&bytes)
            .await
            .inspect_err(|e| tracing::error!("failed to send. :{}", e.to_string()))?;
        tracing::info!("Sent client setup.");

        let mut bytes_mut = receive_stream.receive().await?;
        let message_type = util::get_message_type(&mut bytes_mut)?;
        if message_type == ControlMessageType::ServerSetup {
            tracing::info!("Received server setup.");
            let _ = ServerSetup::decode(&mut bytes_mut)
                .ok_or_else(|| anyhow::anyhow!("Failed to decode server setup."))?;
            Ok(())
        } else {
            tracing::error!("Protocol violation.");
            bail!("Protocol violation.")
        }
    }

    async fn setup_server(
        send_stream: &mut StreamSender<T>,
        receive_stream: &mut StreamReceiver<T>,
    ) -> anyhow::Result<()> {
        tracing::info!("Waiting for server setup.");
        let mut bytes_mut = receive_stream.receive().await?;
        let message_type = util::get_message_type(&mut bytes_mut)?;
        if message_type == ControlMessageType::ClientSetup {
            tracing::info!("Received client setup.");
            let _ = ClientSetup::decode(&mut bytes_mut)
                .ok_or_else(|| anyhow::anyhow!("Failed to decode client setup."))?;
        } else {
            tracing::error!("Protocol violation.");
            bail!("Protocol violation.")
        }
        tracing::info!("Received client setup.");
        let setup_param = SetupParameter {
            path: None,
            max_request_id: 1000,
            authorization_token: vec![],
            authority: None,
            max_auth_token_cache_size: None,
            moq_implementation: Some("MOQ-WASM".to_string()),
        };
        let payload = ServerSetup::new(MOQ_TRANSPORT_VERSION, setup_param).encode();
        let bytes = add_message_type(ControlMessageType::ServerSetup, payload);
        send_stream
            .send(&bytes)
            .await
            .inspect_err(|e| tracing::error!("failed to send. :{}", e.to_string()))
            .inspect(|_| tracing::debug!("ServerSetup has been sent."))
    }

    pub(crate) fn get_request_id(&self) -> u64 {
        let id = self.request_id.load(Ordering::SeqCst);
        tracing::debug!("request_id: {}", id);
        self.request_id.fetch_add(2, Ordering::SeqCst);
        id
    }
}

impl<T: TransportProtocol> Drop for SessionContext<T> {
    fn drop(&mut self) {
        tracing::info!("Session has been dropped.");
        // send goaway
    }
}
