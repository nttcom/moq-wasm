use std::{future::Future, time::Duration};

use anyhow::Result;
use moqt::executor::{self, JoinHandle, MaybeSend};

use super::RelayConnection;

#[cfg(not(test))]
const POLL_INTERVAL: Duration = Duration::from_secs(10);
#[cfg(test)]
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Dropping it stops the refreshes.
pub struct TokenRefreshTask {
    join_handle: JoinHandle,
}

impl TokenRefreshTask {
    /// Every 10 s calls `renew` with the token in force. A different token is
    /// passed to `RelayConnection::refresh_auth_token` with `app_id`; returning
    /// the same token keeps it. A failed `renew` or refresh is tried again at
    /// the next poll. Does nothing for a connection that was given no token.
    pub fn run<F, Fut>(connection: RelayConnection, app_id: String, renew: F) -> Self
    where
        F: Fn(String) -> Fut + MaybeSend + 'static,
        Fut: Future<Output = Result<String>> + MaybeSend + 'static,
    {
        let join_handle = executor::spawn("broadcast-token-refresh", async move {
            let Some(current) = connection.auth_token.clone() else {
                return;
            };
            loop {
                executor::sleep(POLL_INTERVAL).await;
                let in_force = current.lock().await.clone();
                let token = match renew(in_force.clone()).await {
                    Ok(token) if token != in_force => token,
                    Ok(_) => continue,
                    Err(error) => {
                        tracing::warn!(error = %format!("{error:#}"), app_id, "cannot renew the authorization token");
                        continue;
                    }
                };
                match connection.refresh_auth_token(app_id.clone(), token).await {
                    Ok(()) => tracing::info!(app_id, "authorization token refreshed"),
                    Err(error) => {
                        tracing::warn!(error = %format!("{error:#}"), app_id, "authorization token refresh failed");
                    }
                }
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use anyhow::bail;

    use super::*;
    use crate::{
        broadcast::Broadcast,
        relay_connection::MoqtTarget,
        test_support::{FakeRelay, REFUSED_TOKEN, TEST_TIMEOUT},
    };

    const REFRESH_REQUEST: &str = "TRACK_STATUS app/update_auth_token renewed";

    async fn open_connection(relay: &FakeRelay) -> RelayConnection {
        let connection = RelayConnection::new(Some(MoqtTarget {
            url: relay.url.clone(),
            auth_token: Some("secret".to_string()),
            verify_certificate: false,
        }));
        Broadcast::new(connection.clone(), vec!["live".to_string()])
            .announce()
            .await
            .unwrap();
        connection
    }

    /// Returns the tokens `renew` is called with, in order.
    fn run_recording<F>(
        connection: RelayConnection,
        renew: F,
    ) -> (TokenRefreshTask, Arc<Mutex<Vec<String>>>)
    where
        F: Fn(&str, usize) -> Result<String> + Send + 'static,
    {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let task = TokenRefreshTask::run(connection, "app".to_string(), move |current| {
            let call = {
                let mut calls = recorded.lock().unwrap();
                calls.push(current.clone());
                calls.len()
            };
            let renewed = renew(&current, call);
            async move { renewed }
        });
        (task, calls)
    }

    async fn wait_for_calls(calls: &Mutex<Vec<String>>, count: usize) -> Vec<String> {
        tokio::time::timeout(TEST_TIMEOUT, async {
            loop {
                let seen = calls.lock().unwrap().clone();
                if seen.len() >= count {
                    return seen;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
        .await
        .expect("renew must keep being called")
    }

    #[tokio::test]
    async fn a_renewed_token_is_sent_to_the_relay_and_becomes_the_token_in_force() {
        // Arrange
        let mut relay = FakeRelay::spawn("task-renewed");
        let connection = open_connection(&relay).await;

        // Act
        let (_task, calls) = run_recording(connection, |_, _| Ok("renewed".to_string()));
        relay.wait_for_request(REFRESH_REQUEST).await;
        let calls = wait_for_calls(&calls, 4).await;

        // Assert: once in force, the renewed token is not sent again
        assert_eq!(calls[0], "secret");
        assert_eq!(calls[3], "renewed");
        assert!(
            !relay
                .received_requests()
                .contains(&REFRESH_REQUEST.to_string())
        );
    }

    #[tokio::test]
    async fn the_token_in_force_returned_by_renew_is_not_sent() {
        // Arrange
        let mut relay = FakeRelay::spawn("task-unchanged");
        let connection = open_connection(&relay).await;

        // Act
        let (_task, calls) = run_recording(connection, |current, _| Ok(current.to_string()));
        wait_for_calls(&calls, 3).await;

        // Assert
        assert!(
            !relay
                .received_requests()
                .iter()
                .any(|request| request.starts_with("TRACK_STATUS"))
        );
    }

    #[tokio::test]
    async fn a_failed_renew_is_tried_again_at_the_next_poll() {
        // Arrange
        let mut relay = FakeRelay::spawn("task-renew-fails");
        let connection = open_connection(&relay).await;

        // Act
        let (_task, _calls) = run_recording(connection, |_, call| {
            if call == 1 {
                bail!("the issuer is unreachable");
            }
            Ok("renewed".to_string())
        });

        // Assert
        relay.wait_for_request(REFRESH_REQUEST).await;
    }

    #[tokio::test]
    async fn a_token_the_relay_refuses_is_sent_again_at_the_next_poll() {
        // Arrange
        let mut relay = FakeRelay::spawn("task-refused");
        let connection = open_connection(&relay).await;
        let refused = format!("TRACK_STATUS app/update_auth_token {REFUSED_TOKEN}");

        // Act
        let (_task, _calls) = run_recording(connection, |_, _| Ok(REFUSED_TOKEN.to_string()));

        // Assert
        relay.wait_for_request(&refused).await;
        relay.wait_for_request(&refused).await;
    }

    #[tokio::test]
    async fn renew_is_never_called_for_a_connection_that_was_given_no_token() {
        // Arrange
        let relay = FakeRelay::spawn("task-without-token");

        // Act
        let (_task, calls) = run_recording(relay.connection(), |_, _| Ok("renewed".to_string()));
        tokio::time::sleep(POLL_INTERVAL * 10).await;

        // Assert
        assert!(calls.lock().unwrap().is_empty());
    }
}
