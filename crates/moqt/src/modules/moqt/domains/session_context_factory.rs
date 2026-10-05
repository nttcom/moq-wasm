use crate::{
    TransportProtocol,
    modules::moqt::{
        control_plane::{
            constants::{self, MOQ_TRANSPORT_VERSION},
            control_messages::{
                control_message_type::ControlMessageType,
                messages::{
                    client_setup::ClientSetup,
                    parameters::{
                        authorization_token::AuthorizationToken, setup_parameters::SetupParameter,
                    },
                    server_setup::ServerSetup,
                },
            },
        },
        data_plane::stream::{
            bi_stream_sender::BiStreamSender, received_message::ReceivedMessage,
            stream_receiver::BiStreamReceiver,
        },
    },
};

pub(crate) struct SessionContextFactory;

impl SessionContextFactory {
    pub(crate) async fn send_client_setup<T: TransportProtocol>(
        send_stream: &mut BiStreamSender<T>,
        authorization_token: Option<&str>,
    ) -> anyhow::Result<()> {
        let setup_param = SetupParameter {
            path: None,
            max_request_id: 1000,
            authorization_token: authorization_token
                .map(AuthorizationToken::use_value_utf8)
                .into_iter()
                .collect(),
            max_auth_token_cache_size: None,
            authority: None,
            moq_implementation: Some("MOQ-WASM".to_string()),
        };
        let payload =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_param).encode();
        send_stream
            .send(ControlMessageType::ClientSetup, payload)
            .await
            .inspect_err(|e| tracing::error!("failed to send. :{}", e.to_string()))?;
        tracing::info!("Sent client setup.");
        Ok(())
    }

    pub(crate) async fn receive_server_setup<T: TransportProtocol>(
        receive_stream: &mut BiStreamReceiver<T>,
    ) -> anyhow::Result<()> {
        let received_message = match receive_stream.receive().await {
            Ok(Some(b)) => b,
            Ok(None) => {
                tracing::error!("Stream ended before receiving server setup.");
                anyhow::bail!("Stream ended before receiving server setup.")
            }
            Err(error) => {
                tracing::error!(%error, "Stream failed before receiving server setup.");
                anyhow::bail!("Stream failed before receiving server setup: {error}")
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

    pub(crate) async fn receive_client_setup<T: TransportProtocol>(
        receive_stream: &mut BiStreamReceiver<T>,
    ) -> anyhow::Result<ClientSetup> {
        let received_message = match receive_stream.receive().await {
            Ok(Some(b)) => b,
            Ok(None) => {
                tracing::error!("Stream ended before receiving client setup.");
                anyhow::bail!("Stream ended before receiving client setup.")
            }
            Err(error) => {
                tracing::error!(%error, "Stream failed before receiving client setup.");
                anyhow::bail!("Stream failed before receiving client setup: {error}")
            }
        };
        match received_message {
            ReceivedMessage::ClientSetup(client_setup) => {
                tracing::info!(
                    "Received client setup. supported_versions: {:?}",
                    client_setup.supported_versions
                );
                Ok(client_setup)
            }
            _ => {
                tracing::error!("Protocol violation.");
                anyhow::bail!("Protocol violation.")
            }
        }
    }

    pub(crate) async fn send_server_setup<T: TransportProtocol>(
        send_stream: &mut BiStreamSender<T>,
    ) -> anyhow::Result<()> {
        let setup_param = SetupParameter {
            path: None,
            max_request_id: 1000,
            authorization_token: vec![],
            authority: None,
            max_auth_token_cache_size: None,
            moq_implementation: Some("MOQ-WASM".to_string()),
        };
        let payload = ServerSetup::new(MOQ_TRANSPORT_VERSION, setup_param).encode();
        Ok(send_stream
            .send(ControlMessageType::ServerSetup, payload)
            .await
            .inspect_err(|e| tracing::error!("failed to send. :{}", e.to_string()))
            .inspect(|_| tracing::debug!("ServerSetup is sent."))?)
    }
}
