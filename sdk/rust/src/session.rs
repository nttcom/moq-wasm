mod connection_watch_task;
mod token_refresh_task;

use std::{
    ops::Deref,
    sync::{Arc, Mutex},
};

use moqt::{
    AuthorizationTokenSource, CertificateVerification, ClientConfig, DEFAULT_VERSIONS,
    TransportClose, Version, client,
};
use tokio::sync::watch;

use crate::token::{RefreshToken, TokenKeeper};
use connection_watch_task::ConnectionWatchTask;
use token_refresh_task::TokenRefreshTask;

#[derive(Clone, Default)]
pub struct ConnectOptions {
    /// The JWT presented in CLIENT_SETUP.
    pub token: Option<String>,
    /// Called with the current token five minutes before its `exp`, and
    /// every 30 s while it fails, until the token expires; needs `token`.
    pub refresh_token: Option<RefreshToken>,
    pub certificate_verification: CertificateVerification,
    /// The drafts offered, most preferred first; draft-18, then draft-14,
    /// when empty.
    pub versions: Vec<Version>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionEnd {
    /// `Session::close` was called.
    Closed,
    /// The relay connection ended and could not be replaced: retrying
    /// stopped after 60 s, or the relay closed it with a termination code
    /// other than EXPIRED_AUTH_TOKEN.
    ConnectionLost(TransportClose),
    /// The token expired while `RefreshToken` kept failing.
    TokenExpired,
}

/// Connects to `url`: `moqt://` over raw QUIC, `https://` over
/// WebTransport. Fails when the relay cannot be reached or refuses the
/// session, when `refresh_token` is given without `token`, or when `token`
/// is no JWT with an `appId` claim.
pub async fn connect(url: &str, options: ConnectOptions) -> anyhow::Result<Session> {
    let keeper = match (&options.token, options.refresh_token) {
        (Some(token), Some(refresh)) => Some(TokenKeeper::new(token.clone(), refresh)?),
        (None, Some(_)) => anyhow::bail!("refresh_token needs a token"),
        _ => None,
    };
    let current_token = options.token.map(|token| Arc::new(Mutex::new(token)));
    let config = ClientConfig {
        certificate_verification: options.certificate_verification,
        authorization_token: current_token.clone().map(|current| {
            AuthorizationTokenSource::provider(move || {
                let token = current.lock().unwrap().clone();
                async move { Ok(token) }
            })
        }),
        versions: if options.versions.is_empty() {
            DEFAULT_VERSIONS.to_vec()
        } else {
            options.versions
        },
        ..ClientConfig::default()
    };
    let client = Arc::new(client::Session::connect(url, &config).await?);
    let (end_sender, _) = watch::channel(None);
    let end_sender = Arc::new(end_sender);
    let token_refresh = keeper.zip(current_token).map(|(keeper, current_token)| {
        TokenRefreshTask::run(keeper, current_token, client.clone(), end_sender.clone())
    });
    Ok(Session {
        _connection_watch: ConnectionWatchTask::run(client.clone(), end_sender.clone()),
        _token_refresh: token_refresh,
        client,
        end_sender,
    })
}

/// A relay session: the requests of `moqt::client::Session`, through
/// `Deref`, with its token kept fresh. It follows GOAWAY and replaces a lost
/// connection for up to 60 s, sending its namespaces and subscriptions
/// again; when it cannot, the session ends with `SessionEnd::ConnectionLost`.
pub struct Session {
    client: Arc<client::Session>,
    end_sender: Arc<watch::Sender<Option<SessionEnd>>>,
    _connection_watch: ConnectionWatchTask,
    _token_refresh: Option<TokenRefreshTask>,
}

impl Session {
    /// Waits for the session to end; returns at once if it already has.
    pub async fn closed(&self) -> SessionEnd {
        let mut end_receiver = self.end_sender.subscribe();
        let end = end_receiver
            .wait_for(Option::is_some)
            .await
            .map(|end| end.clone());
        // `self` holds the sender, so `wait_for` cannot fail and the fallback is unreachable.
        end.ok().flatten().unwrap_or(SessionEnd::Closed)
    }

    /// Closes the session with NO_ERROR; `closed` resolves with
    /// `SessionEnd::Closed`.
    pub async fn close(&self) {
        end(&self.end_sender, SessionEnd::Closed);
        self.client.close().await;
    }
}

impl Deref for Session {
    type Target = client::Session;

    fn deref(&self) -> &client::Session {
        &self.client
    }
}

fn end(end_sender: &watch::Sender<Option<SessionEnd>>, session_end: SessionEnd) {
    end_sender.send_if_modified(|end| {
        if end.is_some() {
            return false;
        }
        *end = Some(session_end);
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_end_of_a_session_stays() {
        // Arrange
        let (end_sender, end_receiver) = watch::channel(None);

        // Act
        end(&end_sender, SessionEnd::TokenExpired);
        end(&end_sender, SessionEnd::Closed);

        // Assert
        assert_eq!(*end_receiver.borrow(), Some(SessionEnd::TokenExpired));
    }

    #[tokio::test]
    async fn a_refresh_without_a_token_is_refused_before_connecting() {
        // Arrange
        let options = ConnectOptions {
            refresh_token: Some(RefreshToken::new(|token| async { Ok(token) })),
            ..Default::default()
        };

        // Act
        let result = connect("moqt://127.0.0.1:1", options).await;

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("needs a token")));
    }
}
