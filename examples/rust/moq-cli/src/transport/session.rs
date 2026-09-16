use anyhow::{Context, Result, anyhow};
use moqt::{
    ClientConfig, DataReceiver, Endpoint, FilterType, GroupOrder, QUIC, Session, SessionEvent,
    StreamDataReceiverFactory, SubscribeOption,
};
use tracing::info;

use crate::cli::RelayArgs;

const SUBSCRIBER_PRIORITY: u8 = 128;

pub async fn connect_session(relay: &RelayArgs) -> Result<Session<QUIC>> {
    let config = ClientConfig {
        port: 0,
        verify_certificate: !relay.insecure,
        authorization_token: relay.auth_token.clone(),
    };
    let endpoint = Endpoint::<QUIC>::create_client(&config)?;
    info!(relay = %relay.url, "connecting to relay");
    let connecting = endpoint.connect(relay.url.as_str()).await?;
    let session = connecting.await?;
    Ok(session)
}

/// Resolves once the relay ends the session, with the reason as the error.
pub async fn session_closed(session: &Session<QUIC>) -> anyhow::Error {
    loop {
        match session.receive_event().await {
            Ok(SessionEvent::Disconnected()) => return anyhow!("session closed by the relay"),
            Ok(SessionEvent::ProtocolViolation()) => return anyhow!("protocol violation"),
            Ok(_) => {}
            Err(error) => return error.context("session event loop failed"),
        }
    }
}

pub async fn subscribe_track(
    session: &Session<QUIC>,
    namespace: &str,
    name: &str,
) -> Result<StreamDataReceiverFactory<QUIC>> {
    let option = SubscribeOption {
        subscriber_priority: SUBSCRIBER_PRIORITY,
        group_order: GroupOrder::Ascending,
        forward: true,
        filter_type: FilterType::NextGroupStart,
    };
    let subscription = session
        .subscriber()
        .subscribe(namespace.to_string(), name.to_string(), option)
        .await
        .with_context(|| format!("failed to subscribe {namespace}/{name}"))?;
    let receiver = session
        .subscriber()
        .accept_data_receiver(&subscription)
        .await
        .context("failed to accept data receiver")?;
    let DataReceiver::Stream(factory) = receiver else {
        anyhow::bail!("expected stream data receiver");
    };
    Ok(factory)
}
