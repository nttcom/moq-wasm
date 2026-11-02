use std::sync::Arc;

use anyhow::Result;
use moqt::{ClientConfig, Endpoint, QUIC, Session};
use tracing::info;

use super::{auth_token_file::read_auth_token, auth_token_refresh_task::AuthTokenRefreshTask};
use crate::cli::RelayArgs;

pub struct RelayConnection {
    pub session: Arc<Session>,
    auth_token_refresh: Option<AuthTokenRefreshTask>,
}

impl RelayConnection {
    pub async fn close(self) {
        drop(self.auth_token_refresh);
        self.session.close().await;
    }
}

pub async fn connect_relay(relay: &RelayArgs, app_id: &str) -> Result<RelayConnection> {
    let auth_token = match &relay.auth_token_file {
        Some(path) => Some(read_auth_token(path).await?),
        None => relay.auth_token.clone(),
    };
    let config = ClientConfig {
        verify_certificate: !relay.insecure,
        authorization_token: auth_token.clone().map(Into::into),
        ..ClientConfig::default()
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
    })
}
