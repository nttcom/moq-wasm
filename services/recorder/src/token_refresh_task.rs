use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use moqt::{AuthorizationToken, Session};
use serde_json::Value;

use crate::state_sync_task::now_ms;

const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// The relay takes a TRACK_STATUS that carries an AUTHORIZATION TOKEN as a
/// token refresh; it reads neither the namespace `[app id]` nor this name.
const REFRESH_TRACK_NAME: &str = "update_auth_token";
/// A token mint's token is renewed this long before it expires.
const ISSUER_REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);

/// Where the next token comes from.
pub(crate) enum TokenRenewal {
    /// Whatever the file holds, re-read on every poll.
    File(PathBuf),
    /// A temporary app's token, renewed by `POST /tokens/refresh` on the
    /// token mint at this URL shortly before it expires.
    Issuer(String),
}

impl TokenRenewal {
    /// `None` keeps `current`.
    async fn next(&self, current: &str) -> Result<Option<String>> {
        let token = match self {
            Self::File(path) => read_token_file(path).await?,
            Self::Issuer(url) => {
                if !expires_within(current, ISSUER_REFRESH_MARGIN, now_ms() / 1000) {
                    return Ok(None);
                }
                refresh_at_issuer(url, current).await?
            }
        };
        Ok((token != current).then_some(token))
    }
}

/// Lets a recording outlive the token it connected with.
pub(crate) struct TokenRefreshTask {
    join_handle: tokio::task::JoinHandle<()>,
}

impl TokenRefreshTask {
    pub(crate) fn run(
        session: Arc<Session>,
        renewal: TokenRenewal,
        app_id: String,
        mut current_token: String,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(POLL_INTERVAL);
            loop {
                interval.tick().await;
                let token = match renewal.next(&current_token).await {
                    Ok(Some(token)) => token,
                    Ok(None) => continue,
                    Err(error) => {
                        tracing::warn!(
                            error = format!("{error:#}"),
                            "keeping the current authorization token"
                        );
                        continue;
                    }
                };
                match session
                    .subscriber()
                    .track_status(
                        app_id.clone(),
                        REFRESH_TRACK_NAME.to_string(),
                        vec![AuthorizationToken::use_value_utf8(&token)],
                    )
                    .await
                {
                    Ok(_) => tracing::info!("authorization token refreshed"),
                    Err(error) => tracing::warn!(%error, "authorization token refresh rejected"),
                }
                current_token = token;
            }
        });
        Self { join_handle }
    }
}

impl Drop for TokenRefreshTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

/// A token without a readable `exp` counts as expiring, so it is renewed.
fn expires_within(token: &str, margin: Duration, now: u64) -> bool {
    auth_token::decode_unverified_payload(token)
        .and_then(|claims| claims.get("exp").and_then(Value::as_u64))
        .is_none_or(|exp| exp <= now + margin.as_secs())
}

async fn refresh_at_issuer(url: &str, token: &str) -> Result<String> {
    let response = http_client::client()
        .post(format!("{}/tokens/refresh", url.trim_end_matches('/')))
        .timeout(Duration::from_secs(10))
        .json(&serde_json::json!({ "token": token }))
        .send()
        .await
        .context("cannot reach the token mint")?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        bail!("the token mint answered {status}: {}", body["error"]);
    }
    body["token"]
        .as_str()
        .map(str::to_string)
        .context("the token mint answered without a token")
}

/// The file holds one JWT; surrounding whitespace, such as a trailing newline,
/// is ignored.
pub(crate) async fn read_token_file(path: &Path) -> Result<String> {
    let content = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("cannot read the token file {}", path.display()))?;
    let token = content.trim();
    if token.is_empty() {
        bail!("token file {} is empty", path.display());
    }
    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_file(name: &str, content: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("recorder-{name}-{}", std::process::id()));
        std::fs::write(&path, content).unwrap();
        path
    }

    fn token_expiring_at(exp: u64) -> String {
        auth_token::sign_token(&serde_json::json!({ "appId": "temp-a", "exp": exp }), "s").unwrap()
    }

    #[test]
    fn an_issuer_token_is_renewed_only_within_the_margin() {
        // Arrange
        let margin = Duration::from_secs(300);

        // Act / Assert
        assert!(!expires_within(&token_expiring_at(1_000), margin, 600));
        assert!(expires_within(&token_expiring_at(1_000), margin, 700));
        assert!(expires_within("not-a-jwt", margin, 0));
    }

    #[tokio::test]
    async fn reads_the_trimmed_token() {
        // Arrange
        let path = token_file("token-trimmed", "  jwt-value\n");

        // Act
        let token = read_token_file(&path).await;

        // Assert
        assert_eq!(token.unwrap(), "jwt-value");
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn rejects_an_empty_file() {
        // Arrange
        let path = token_file("token-empty", " \n");

        // Act
        let token = read_token_file(&path).await;

        // Assert
        assert!(token.is_err());
        std::fs::remove_file(path).unwrap();
    }
}
