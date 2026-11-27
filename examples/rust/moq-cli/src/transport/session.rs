use std::path::PathBuf;

use anyhow::{Result, anyhow, ensure};
use moq_wasm::{CertificateVerification, ConnectOptions, RefreshToken, Session};
use tracing::info;

use super::auth_token_file::{current_auth_token, read_auth_token};
use crate::cli::RelayArgs;

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

fn refresh_from_file(path: PathBuf) -> RefreshToken {
    RefreshToken::new(move |current| {
        let path = path.clone();
        async move {
            let token = read_auth_token(&path).await?;
            ensure!(
                token != current,
                "auth token file {} is not rewritten yet",
                path.display()
            );
            Ok(token)
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
