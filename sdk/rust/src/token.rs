use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use auth_token::Claims;

/// A token is refreshed this long before it expires, as the browser pages
/// and the Python SDK do.
const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

type RefreshFuture = Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send>>;

/// Gets a later token of the same app, given the current one, typically
/// from the application's app server.
#[derive(Clone)]
pub struct RefreshToken(Arc<dyn Fn(String) -> RefreshFuture + Send + Sync>);

impl RefreshToken {
    pub fn new<F, Fut>(refresh: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<String>> + Send + 'static,
    {
        Self(Arc::new(move |token| Box::pin(refresh(token))))
    }
}

/// The claims the session reads from its token, without verifying it: the
/// relay verifies the token, and the session only needs to know when to
/// refresh it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TokenClaims {
    pub(crate) app_id: String,
    /// Unix seconds; `None` for a token that does not expire.
    pub(crate) expires_at: Option<u64>,
}

impl TokenClaims {
    pub(crate) fn read(token: &str) -> anyhow::Result<Self> {
        let payload =
            auth_token::decode_unverified_payload(token).context("the token is not a JWT")?;
        let claims: Claims =
            serde_json::from_value(payload.into()).context("the token has no appId claim")?;
        Ok(Self {
            app_id: claims.app_id,
            expires_at: claims.exp,
        })
    }
}

/// The token expired before `RefreshToken` returned a later one.
#[derive(Debug)]
pub(crate) struct TokenExpired;

/// Refreshes a session's token `REFRESH_MARGIN` before it expires, retrying
/// every `RETRY_INTERVAL` until it expires.
pub(crate) struct TokenKeeper {
    token: String,
    claims: TokenClaims,
    refresh: RefreshToken,
}

impl TokenKeeper {
    pub(crate) fn new(token: String, refresh: RefreshToken) -> anyhow::Result<Self> {
        Ok(Self {
            claims: TokenClaims::read(&token)?,
            token,
            refresh,
        })
    }

    /// Calls `hand_over` with each new token and its app ID. Returns
    /// `Ok(())` once the current token does not expire, and `Err` once it
    /// expired unrefreshed.
    pub(crate) async fn keep_fresh<F, Fut>(mut self, hand_over: F) -> Result<(), TokenExpired>
    where
        F: Fn(String, String) -> Fut,
        Fut: Future<Output = ()>,
    {
        while let Some(expires_at) = self.claims.expires_at {
            tokio::time::sleep(refresh_delay(expires_at, unix_now())).await;
            self.renew().await?;
            hand_over(self.token.clone(), self.claims.app_id.clone()).await;
        }
        Ok(())
    }

    async fn renew(&mut self) -> Result<(), TokenExpired> {
        loop {
            let refreshed = (self.refresh.0)(self.token.clone())
                .await
                .and_then(|token| Ok((TokenClaims::read(&token)?, token)));
            match refreshed {
                Ok((claims, token)) => {
                    self.claims = claims;
                    self.token = token;
                    return Ok(());
                }
                Err(error) => {
                    let app_id = &self.claims.app_id;
                    if self.claims.expires_at.is_some_and(|expires_at| {
                        unix_now() + RETRY_INTERVAL.as_secs() >= expires_at
                    }) {
                        tracing::error!(error = %format!("{error:#}"), app_id, "the token expired");
                        return Err(TokenExpired);
                    }
                    tracing::warn!(error = %format!("{error:#}"), app_id, "cannot refresh the token yet");
                    tokio::time::sleep(RETRY_INTERVAL).await;
                }
            }
        }
    }
}

fn refresh_delay(expires_at: u64, now: u64) -> Duration {
    Duration::from_secs(
        expires_at
            .saturating_sub(REFRESH_MARGIN.as_secs())
            .saturating_sub(now),
    )
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use auth_token::sign_token;

    use super::*;

    fn token(app_id: &str, exp: Option<u64>) -> String {
        let claims = Claims {
            app_id: app_id.to_string(),
            publish: None,
            subscribe: None,
            bots: None,
            iat: None,
            exp,
            relay_id: None,
        };
        sign_token(&claims, "secret").unwrap()
    }

    #[test]
    fn the_claims_are_read_without_the_secret() {
        // Arrange
        let token = token("app", Some(1_000));

        // Act
        let claims = TokenClaims::read(&token).unwrap();

        // Assert
        assert_eq!(
            claims,
            TokenClaims {
                app_id: "app".to_string(),
                expires_at: Some(1_000),
            }
        );
    }

    #[test]
    fn what_is_not_a_token_is_refused() {
        // Act / Assert
        assert!(TokenClaims::read("not a token").is_err());
    }

    #[test]
    fn a_token_is_refreshed_five_minutes_before_it_expires_or_at_once() {
        // Act / Assert
        assert_eq!(refresh_delay(1_000, 600), Duration::from_secs(100));
        assert_eq!(refresh_delay(1_000, 900), Duration::ZERO);
    }

    #[tokio::test]
    async fn each_refreshed_token_is_handed_over() {
        // Arrange: the first token is due now, the second never expires
        let first = token("app", Some(unix_now() + 60));
        let second = token("app", None);
        let refreshed = second.clone();
        let keeper = TokenKeeper::new(
            first,
            RefreshToken::new(move |_| {
                let refreshed = refreshed.clone();
                async move { Ok(refreshed) }
            }),
        )
        .unwrap();
        let handed_over = Mutex::new(Vec::new());

        // Act
        let result = keeper
            .keep_fresh(|token, app_id| {
                handed_over.lock().unwrap().push((token, app_id));
                async {}
            })
            .await;

        // Assert
        assert!(result.is_ok());
        assert_eq!(
            handed_over.into_inner().unwrap(),
            vec![(second, "app".to_string())]
        );
    }

    #[tokio::test]
    async fn the_keeper_fails_once_the_token_expires_while_refreshing_fails() {
        // Arrange: the token expires within the retry interval
        let keeper = TokenKeeper::new(
            token("app", Some(unix_now() + 10)),
            RefreshToken::new(|_| async { anyhow::bail!("app server down") }),
        )
        .unwrap();

        // Act
        let result = keeper.keep_fresh(|_, _| async {}).await;

        // Assert
        assert!(result.is_err());
    }
}
