use std::sync::atomic::AtomicU64;

use crate::{
    SessionEvent, TransportProtocol,
    modules::moqt::{
        control_plane::{
            constants::{self, MOQ_TRANSPORT_VERSION},
            control_messages::{
                control_message_type::ControlMessageType,
                messages::{
                    client_setup::ClientSetup, parameters::setup_parameters::SetupParameter,
                    server_setup::ServerSetup,
                },
            },
            utils::add_message_type,
        },
        data_plane::streams::stream::{
            received_message::ReceivedMessage, stream_receiver::BiStreamReceiver,
            stream_sender::StreamSender,
        },
        domains::session_context::SessionContext,
    },
};

pub(crate) struct SessionContextFactory;

impl SessionContextFactory {
    pub(crate) async fn client<T: TransportProtocol>(
        transport_connection: T::Connection,
        mut send_stream: StreamSender<T>,
        receive_stream: &mut BiStreamReceiver<T>,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent<T>>,
    ) -> anyhow::Result<SessionContext<T>> {
        Self::setup_client(&mut send_stream, receive_stream).await?;

        Ok(SessionContext::new(
            transport_connection,
            send_stream,
            AtomicU64::new(1),
            event_sender,
        ))
    }

    pub(crate) async fn server<T: TransportProtocol>(
        transport_connection: T::Connection,
        mut send_stream: StreamSender<T>,
        receive_stream: &mut BiStreamReceiver<T>,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent<T>>,
    ) -> anyhow::Result<SessionContext<T>> {
        Self::setup_server(&mut send_stream, receive_stream).await?;

        Ok(SessionContext::new(
            transport_connection,
            send_stream,
            AtomicU64::new(1),
            event_sender,
        ))
    }

    async fn setup_client<T: TransportProtocol>(
        send_stream: &mut StreamSender<T>,
        receive_stream: &mut BiStreamReceiver<T>,
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

        let received_message = match receive_stream.receive().await {
            Some(Ok(b)) => b,
            _ => {
                tracing::error!("Stream ended.");
                anyhow::bail!("Stream ended.")
            }
        };
        match received_message {
            ReceivedMessage::ServerSetup(server_setup) => {
                tracing::info!(
                    "Received server setup. selected_version: {}",
                    server_setup.selected_version
                );
                Ok(())
            }
            _ => {
                tracing::error!("Protocol violation.");
                anyhow::bail!("Protocol violation.")
            }
        }
    }

    async fn setup_server<T: TransportProtocol>(
        send_stream: &mut StreamSender<T>,
        receive_stream: &mut BiStreamReceiver<T>,
    ) -> anyhow::Result<()> {
        tracing::info!("Waiting for server setup.");
        let received_message = match receive_stream.receive().await {
            Some(Ok(b)) => b,
            _ => {
                tracing::error!("Stream ended.");
                anyhow::bail!("Stream ended.")
            }
        };
        match received_message {
            ReceivedMessage::ClientSetup(client_setup) => {
                tracing::info!(
                    "Received client setup. supported_versions: {:?}",
                    client_setup.supported_versions
                );
            }
            _ => {
                tracing::error!("Protocol violation.");
                anyhow::bail!("Protocol violation.")
            }
        };
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
            .inspect(|_| tracing::debug!("ServerSetup is sent."))
    }
}
