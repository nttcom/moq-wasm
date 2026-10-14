//! The draft-18 SETUP exchange: each endpoint opens a unidirectional
//! control stream that begins with SETUP (§3.3, §10.3).

use super::{
    ControlChannel,
    message_stream::{MessageDecoder, MessageReceiver, MessageSender},
    messages::{Message, SetupMessage, encode_setup},
};
use crate::modules::{
    moqt::control_plane::control_messages::messages::{
        parameters::{authorization_token::AuthorizationToken, setup_parameters::SetupParameter},
        server_setup::ServerSetup,
    },
    transport::{
        connect_target::{ClientTransport, ConnectTarget},
        transport_connection::BoxedConnection,
    },
};

const IMPLEMENTATION: &str = "MOQ-WASM";
/// draft-18 negotiates its version through ALPN and carries no version in
/// SETUP; the model's SERVER_SETUP names it with the draft's number.
const DRAFT18_VERSION: u32 = 0xff00_0012;

async fn receive_setup(peer: &mut MessageReceiver) -> anyhow::Result<SetupMessage> {
    match peer.receive().await {
        Ok(Some(Message::Setup(setup))) => Ok(setup),
        Ok(Some(_)) => {
            anyhow::bail!("Protocol violation: the control stream does not begin with SETUP")
        }
        Ok(None) => anyhow::bail!("Stream ended before receiving SETUP."),
        Err(error) => anyhow::bail!("Stream failed before receiving SETUP: {error}"),
    }
}

/// §10.3.1.1 / §10.3.1.2: over native QUIC the client states the URI's
/// authority and path; over WebTransport the CONNECT request carries them.
fn client_setup(target: &ConnectTarget, authorization_token: Option<&str>) -> SetupMessage {
    let native_quic = target.transport == ClientTransport::Quic;
    let path = native_quic.then(|| match target.url.query() {
        Some(query) => format!("{}?{query}", target.url.path()),
        None => target.url.path().to_string(),
    });
    SetupMessage {
        path,
        authority: native_quic.then(|| target.url.authority().to_string()),
        authorization_tokens: authorization_token
            .map(AuthorizationToken::use_value_utf8)
            .into_iter()
            .collect(),
        max_auth_token_cache_size: 0,
        implementation: Some(IMPLEMENTATION.to_string()),
    }
}

fn server_setup(setup: SetupMessage) -> ServerSetup {
    ServerSetup::new(
        DRAFT18_VERSION,
        SetupParameter {
            path: setup.path,
            max_request_id: 0,
            authorization_token: setup.authorization_tokens,
            max_auth_token_cache_size: Some(setup.max_auth_token_cache_size),
            authority: setup.authority,
            moq_implementation: setup.implementation,
        },
    )
}

/// Sends the client's SETUP and reads the server's.
pub(crate) async fn connect(
    transport_connection: &BoxedConnection,
    authorization_token: Option<&str>,
    target: &ConnectTarget,
) -> anyhow::Result<(ControlChannel, MessageReceiver, ServerSetup)> {
    let control = MessageSender::new(transport_connection.open_uni().await?);
    control
        .send(&encode_setup(&client_setup(target, authorization_token))?)
        .await?;
    let mut peer = MessageReceiver::new(transport_connection.accept_uni().await?, MessageDecoder);
    let setup = receive_setup(&mut peer).await?;
    tracing::debug!(?setup.implementation, "Received SETUP");
    Ok((ControlChannel::new(control, 0), peer, server_setup(setup)))
}

/// Reads the client's SETUP; the server's waits for `ServerHandshake::accept`.
pub(crate) async fn accept(
    transport_connection: &BoxedConnection,
) -> anyhow::Result<ServerHandshake> {
    let mut peer = MessageReceiver::new(transport_connection.accept_uni().await?, MessageDecoder);
    let client_setup = receive_setup(&mut peer).await?;
    tracing::debug!(?client_setup.implementation, "Received SETUP");
    Ok(ServerHandshake { client_setup, peer })
}

pub(crate) struct ServerHandshake {
    pub(crate) client_setup: SetupMessage,
    peer: MessageReceiver,
}

impl ServerHandshake {
    pub(crate) async fn accept(
        self,
        transport_connection: &BoxedConnection,
    ) -> anyhow::Result<(ControlChannel, MessageReceiver)> {
        let control = MessageSender::new(transport_connection.open_uni().await?);
        let setup = SetupMessage {
            implementation: Some(IMPLEMENTATION.to_string()),
            ..SetupMessage::default()
        };
        control.send(&encode_setup(&setup)?).await?;
        Ok((ControlChannel::new(control, 1), self.peer))
    }
}

#[cfg(test)]
mod tests {
    use super::client_setup;
    use crate::modules::transport::connect_target::ConnectTarget;

    #[test]
    fn a_native_quic_client_states_the_authority_and_path() {
        // Arrange
        let target = ConnectTarget::parse("moqt://relay.example:4443/moq?room=1").unwrap();

        // Act
        let setup = client_setup(&target, Some("jwt"));

        // Assert
        assert_eq!(setup.authority.as_deref(), Some("relay.example:4443"));
        assert_eq!(setup.path.as_deref(), Some("/moq?room=1"));
        assert_eq!(setup.authorization_tokens.len(), 1);
    }

    #[test]
    fn a_web_transport_client_leaves_authority_and_path_to_connect() {
        // Arrange
        let target = ConnectTarget::parse("https://relay.example/moq").unwrap();

        // Act
        let setup = client_setup(&target, None);

        // Assert
        assert_eq!(setup.authority, None);
        assert_eq!(setup.path, None);
    }
}
