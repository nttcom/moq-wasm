use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use moqt::{AuthorizationToken, Session};

const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// The relay takes a TRACK_STATUS that carries an AUTHORIZATION TOKEN as a
/// token refresh; it reads neither the namespace `[app id]` nor this name.
const REFRESH_TRACK_NAME: &str = "update_auth_token";

/// Lets a recording outlive the token it connected with.
pub(crate) struct TokenRefreshTask {
    join_handle: tokio::task::JoinHandle<()>,
}

impl TokenRefreshTask {
    pub(crate) fn run(
        session: Arc<Session>,
        token_file: PathBuf,
        app_id: String,
        mut current_token: String,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(POLL_INTERVAL);
            loop {
                interval.tick().await;
                let token = match read_token_file(&token_file).await {
                    Ok(token) => token,
                    Err(error) => {
                        tracing::warn!(%error, "keeping the current authorization token");
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
