use std::{path::PathBuf, sync::Arc, time::Duration};

use moqt::Session;
use tokio::sync::Mutex;
use tracing::{info, warn};

use super::auth_token_file::read_auth_token;

const POLL_INTERVAL: Duration = Duration::from_secs(10);

pub struct AuthTokenRefreshTask {
    join_handle: tokio::task::JoinHandle<()>,
}

impl AuthTokenRefreshTask {
    /// `current` is the token in force, which the session's
    /// `AuthorizationTokenSource` sends on a connection opened after this one.
    pub fn run(
        session: Arc<Session>,
        token_file: PathBuf,
        app_id: String,
        current: Arc<Mutex<String>>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
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
                if token == *current.lock().await {
                    continue;
                }
                match session
                    .subscriber()
                    .refresh_authorization_token(app_id.clone(), &token)
                    .await
                {
                    Ok(_) => {
                        info!("authorization token refreshed");
                        *current.lock().await = token;
                    }
                    Err(error) => warn!(%error, "authorization token refresh rejected"),
                }
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
