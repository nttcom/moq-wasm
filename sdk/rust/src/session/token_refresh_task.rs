use std::sync::{Arc, Mutex};

use moqt::{
    client,
    executor::{self, AbortOnDrop},
};
use tokio::sync::watch;

use super::{SessionEnd, end};
use crate::token::TokenKeeper;

/// Makes each new token the one the session's next connection presents and
/// sends it to the relay (docs/architecture/relay/architecture.md "Token
/// refresh"). Ends the session with `TokenExpired` when the token expired
/// unrefreshed.
pub(super) struct TokenRefreshTask {
    _join_handle: AbortOnDrop,
}

impl TokenRefreshTask {
    pub(super) fn run(
        keeper: TokenKeeper,
        current_token: Arc<Mutex<String>>,
        client: Arc<client::Session>,
        end_sender: Arc<watch::Sender<Option<SessionEnd>>>,
    ) -> Self {
        let join_handle = executor::spawn("moq-wasm-token-refresh", async move {
            let kept = keeper
                .keep_fresh(|token, app_id| {
                    *current_token.lock().unwrap() = token.clone();
                    let client = client.clone();
                    async move {
                        // A relay that refuses it keeps the connection's token
                        // until it expires; the next connection presents this one.
                        if let Err(error) = client
                            .moqt_session()
                            .subscriber()
                            .refresh_authorization_token(app_id.clone(), &token)
                            .await
                        {
                            tracing::warn!(?error, app_id, "the relay refused the refreshed token");
                        }
                    }
                })
                .await;
            if kept.is_err() {
                end(&end_sender, SessionEnd::TokenExpired);
                client.close().await;
            }
        })
        .abort_on_drop();
        Self {
            _join_handle: join_handle,
        }
    }
}
