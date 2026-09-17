use std::{path::PathBuf, sync::Arc, time::Duration};

use moqt::{QUIC, Session, wire::AuthorizationToken};
use tracing::{info, warn};

use super::auth_token_file::read_auth_token;

const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Namespace `[appId]` and this track name are the convention the relay
/// accepts as a token refresh; it reads neither.
const REFRESH_TRACK_NAME: &str = "update_auth_token";

pub struct AuthTokenRefreshTask {
    join_handle: tokio::task::JoinHandle<()>,
}

impl AuthTokenRefreshTask {
    pub fn run(
        session: Arc<Session<QUIC>>,
        token_file: PathBuf,
        app_id: String,
        current_token: String,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut current_token = current_token;
            let mut interval = tokio::time::interval(POLL_INTERVAL);
            loop {
                interval.tick().await;
                let token = match read_auth_token(&token_file).await {
                    Ok(token) => token,
                    Err(error) => {
                        warn!(%error, "auth token file is unreadable; keeping the current token");
                        continue;
                    }
                };
                if token == current_token {
                    continue;
                }
                match session
                    .subscriber()
                    .track_status(
                        app_id.clone(),
                        REFRESH_TRACK_NAME.to_string(),
                        vec![AuthorizationToken::use_value_utf8(&token)],
                    )
                    .await
                {
                    Ok(_) => info!("authorization token refreshed"),
                    Err(error) => warn!(%error, "authorization token refresh rejected"),
                }
                current_token = token;
            }
        });
        Self { join_handle }
    }
}

impl Drop for AuthTokenRefreshTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
