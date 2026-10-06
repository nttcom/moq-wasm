use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use moqt::{
    ClientConfig, DataReceiver, Endpoint, FilterType, GroupOrder, QUIC, Session, SessionEvent,
    StreamDataReceiverFactory, SubscribeOption,
};
use tracing::info;

use super::{auth_token_file::read_auth_token, auth_token_refresh_task::AuthTokenRefreshTask};
use crate::cli::RelayArgs;

const SUBSCRIBER_PRIORITY: u8 = 128;

pub struct RelayConnection {
    pub session: Arc<Session>,
    auth_token_refresh: Option<AuthTokenRefreshTask>,
    endpoint: Endpoint<QUIC>,
}

impl RelayConnection {
    /// Every other handle to the session (publishers, track writers, clones of
    /// `session`) must already be dropped, or the wait never ends.
    pub async fn close(self) {
        drop(self.auth_token_refresh);
        drop(self.session);
        self.endpoint.wait_idle().await;
    }
}

pub async fn connect_relay(relay: &RelayArgs, app_id: &str) -> Result<RelayConnection> {
    let auth_token = match &relay.auth_token_file {
        Some(path) => Some(read_auth_token(path).await?),
        None => relay.auth_token.clone(),
    };
    let config = ClientConfig {
        port: 0,
        verify_certificate: !relay.insecure,
        authorization_token: auth_token.clone(),
    };
    let endpoint = Endpoint::<QUIC>::create_client(&config)?;
    info!(relay = %relay.url, "connecting to relay");
    let connecting = endpoint.connect(relay.url.as_str()).await?;
    let session = Arc::new(connecting.await?);
    let auth_token_refresh = match (&relay.auth_token_file, auth_token) {
        (Some(path), Some(token)) => Some(AuthTokenRefreshTask::run(
            session.clone(),
            path.clone(),
            app_id.to_string(),
            token,
        )),
        _ => None,
    };
    Ok(RelayConnection {
        session,
        auth_token_refresh,
        endpoint,
    })
}

/// Resolves once the relay ends the session, with the reason as the error.
pub async fn session_closed(session: &Session) -> anyhow::Error {
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
    session: &Session,
    namespace: &str,
    name: &str,
) -> Result<StreamDataReceiverFactory> {
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
