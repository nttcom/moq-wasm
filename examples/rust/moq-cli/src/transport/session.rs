use std::sync::Arc;

use anyhow::Result;
use moqt::{
    AuthorizationTokenSource, CertificateVerification, ClientConfig, Endpoint, QUIC, Session,
};
use tokio::sync::Mutex;
use tracing::info;

use super::{auth_token_file::current_auth_token, auth_token_refresh_task::AuthTokenRefreshTask};
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
    let auth_token = current_auth_token(relay).await?;
    let current = auth_token.clone().map(|token| Arc::new(Mutex::new(token)));
    let authorization_token = match (&relay.auth_token_file, &current) {
        (Some(_), Some(current)) => Some(AuthorizationTokenSource::shared(Arc::clone(current))),
        _ => auth_token.map(Into::into),
    };
    let config = ClientConfig {
        certificate_verification: CertificateVerification::native_roots_or_disabled(
            !relay.insecure,
        ),
        authorization_token,
        versions: relay.offered_versions(),
        ..ClientConfig::default()
    };
    let endpoint = Endpoint::<QUIC>::create_client(&config)?;
    info!(relay = %relay.url, "connecting to relay");
    let connecting = endpoint.connect(relay.url.as_str()).await?;
    let session = Arc::new(connecting.await?);
    let auth_token_refresh = match (&relay.auth_token_file, current) {
        (Some(path), Some(current)) => Some(AuthTokenRefreshTask::run(
            session.clone(),
            path.clone(),
            app_id.to_string(),
            current,
        )),
        _ => None,
    };
    Ok(RelayConnection {
        session,
        auth_token_refresh,
    })
}
