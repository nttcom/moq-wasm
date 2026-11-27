use std::{path::PathBuf, time::Duration};

use anyhow::{Result, anyhow};
use moq_wasm::{CertificateVerification, ConnectOptions, RefreshToken, Session};
use tracing::{info, warn};

use super::auth_token_file::{current_auth_token, read_auth_token};
use crate::cli::RelayArgs;

const POLL_INTERVAL: Duration = Duration::from_secs(10);

pub async fn connect_relay(relay: &RelayArgs) -> Result<Session> {
    let options = ConnectOptions {
        token: current_auth_token(relay).await?,
        refresh_token: relay.auth_token_file.clone().map(refresh_from_file),
        certificate_verification: CertificateVerification::native_roots_or_disabled(
            !relay.insecure,
        ),
        versions: relay.versions.clone(),
    };
    info!(url = %relay.url, "connecting to relay");
    moq_wasm::connect(relay.url.as_str(), options).await
}

/// The session asks for a later token 5 minutes before the current one
/// expires. The file is polled until it holds a new token rather than failing
/// at once: the SDK gives up on a token with under 30 s left after one failure,
/// and the file may be rewritten later than that.
fn refresh_from_file(path: PathBuf) -> RefreshToken {
    RefreshToken::new(move |current| {
        let path = path.clone();
        async move {
            let mut interval = tokio::time::interval(POLL_INTERVAL);
            loop {
                interval.tick().await;
                match read_auth_token(&path).await {
                    Ok(token) if token != current => {
                        info!("auth token file rewritten");
                        return Ok(token);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        warn!(%error, "auth token file is unreadable; keeping the current token")
                    }
                }
            }
        }
    })
}

pub async fn run_until_closed(
    session: &Session,
    work: impl Future<Output = Result<()>>,
) -> Result<()> {
    let result = tokio::select! {
        result = work => result,
        end = session.closed() => Err(anyhow!("the relay session ended: {end:?}")),
    };
    session.close().await;
    result
}
