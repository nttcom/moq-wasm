use crate::modules::moqt::{
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
};

pub(crate) struct SessionContextFactory;

impl SessionContextFactory {
    pub(crate) async fn send_client_setup(
        send_stream: &mut BiStreamSender,
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
            .await?;
        tracing::debug!("Sent client setup.");
        Ok(())
    }

    pub(crate) async fn receive_server_setup(
        receive_stream: &mut BiStreamReceiver,
    ) -> anyhow::Result<ServerSetup> {
        let received_message = match receive_stream.receive().await {
            Ok(Some(b)) => b,
            Ok(None) => {
                anyhow::bail!("Stream ended before receiving server setup.")
            }
            Err(error) => {
                anyhow::bail!("Stream failed before receiving server setup: {error}")
            }
        };
        match received_message {
            ReceivedMessage::ServerSetup(server_setup) => {
                tracing::debug!(
                    selected_version = server_setup.selected_version,
                    "Received server setup"
                );
                Ok(server_setup)
            }
            _ => anyhow::bail!("Protocol violation: the first control message is not SERVER_SETUP"),
        }
    }

    pub(crate) async fn receive_client_setup(
        receive_stream: &mut BiStreamReceiver,
    ) -> anyhow::Result<ClientSetup> {
        let received_message = match receive_stream.receive().await {
            Ok(Some(b)) => b,
            Ok(None) => {
                anyhow::bail!("Stream ended before receiving client setup.")
            }
            Err(error) => {
                anyhow::bail!("Stream failed before receiving client setup: {error}")
            }
        };
        match received_message {
            ReceivedMessage::ClientSetup(client_setup) => {
                tracing::debug!(
                    supported_versions = ?client_setup.supported_versions,
                    "Received client setup"
                );
                Ok(client_setup)
            }
            _ => anyhow::bail!("Protocol violation: the first control message is not CLIENT_SETUP"),
        }
    }

    pub(crate) async fn send_server_setup(send_stream: &mut BiStreamSender) -> anyhow::Result<()> {
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
            .inspect(|_| tracing::debug!("ServerSetup is sent."))?)
    }
}
