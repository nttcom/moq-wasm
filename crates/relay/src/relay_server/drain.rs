use std::{sync::Arc, time::Duration};

use moqt::TerminationErrorCode;

use crate::modules::{
    cascading::route_registry::RelayRouteRegistry, domain::session_peer::SessionPeer,
    session::session_repository::SessionRepository,
};

/// Session migration, draft-14 §3.5: the relay leaves the route registry's
/// active set, tells every client session where to reconnect with GOAWAY and
/// waits for the clients to close. Clients still connected after `timeout`
/// are closed with GOAWAY_TIMEOUT. Relay-peer sessions receive a GOAWAY with
/// no URI, which makes the other relay subscribe the remaining publishers of
/// the tracks this relay feeds it; nothing waits for them.
pub(crate) async fn drain_client_sessions(
    repo: &Arc<tokio::sync::Mutex<SessionRepository>>,
    route_registry: &dyn RelayRouteRegistry,
    timeout: Duration,
) {
    if let Err(error) = route_registry.mark_draining().await {
        tracing::warn!(%error, "failed to mark this relay as draining in the route registry");
    }
    let target = match route_registry.find_migration_target().await {
        Ok(target) => target,
        Err(error) => {
            tracing::warn!(%error, "failed to look up a migration target; clients reuse the current URI");
            None
        }
    };

    let (sessions, relay_sessions, mut client_session_count) = {
        let repo = repo.lock().await;
        (
            repo.sessions_with_peer(SessionPeer::Client),
            repo.sessions_with_peer(SessionPeer::Relay),
            repo.client_session_count(),
        )
    };
    tracing::info!(
        client_sessions = sessions.len(),
        relay_sessions = relay_sessions.len(),
        migration_target = target.as_deref().unwrap_or(""),
        timeout_secs = timeout.as_secs(),
        "sending GOAWAY to every session"
    );
    for session in &sessions {
        let new_session_uri = target
            .as_deref()
            .map(|host| format!("{}://{host}", session.url_scheme()))
            .unwrap_or_default();
        if let Err(error) = session.go_away(new_session_uri).await {
            tracing::warn!(%error, "failed to send GOAWAY");
        }
    }
    for session in &relay_sessions {
        if let Err(error) = session.go_away(String::new()).await {
            tracing::warn!(%error, "failed to send GOAWAY to a relay peer");
        }
    }

    let all_clients_left = tokio::time::timeout(timeout, async {
        while *client_session_count.borrow_and_update() > 0 {
            if client_session_count.changed().await.is_err() {
                break;
            }
        }
    })
    .await
    .is_ok();
    if all_clients_left {
        tracing::info!("every client session has left");
        return;
    }

    let remaining = repo.lock().await.sessions_with_peer(SessionPeer::Client);
    tracing::warn!(
        remaining = remaining.len(),
        "drain timed out; closing the remaining client sessions with GOAWAY_TIMEOUT"
    );
    for session in remaining {
        session.close(
            TerminationErrorCode::GoawayTimeout,
            "the relay is shutting down",
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use moqt::TerminationErrorCode;

    use super::drain_client_sessions;
    use crate::modules::{
        auth::verified_token::VerifiedToken,
        domain::session_peer::SessionPeer,
        session::session_repository::SessionRepository,
        test_support::{
            mock_session::{RecordedControlMessages, mock_new_session},
            stub_route_registry::StubRouteRegistry,
        },
    };

    const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

    async fn repository_with_client_session(
        session_id: u64,
    ) -> (
        Arc<tokio::sync::Mutex<SessionRepository>>,
        RecordedControlMessages,
    ) {
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let repository = Arc::new(tokio::sync::Mutex::new(SessionRepository::new()));
        let (new_session, recorded) = mock_new_session(session_id, VerifiedToken::full_access());
        repository.lock().await.add(new_session, sender).await;
        (repository, recorded)
    }

    #[tokio::test]
    async fn drain_marks_the_relay_draining_and_returns_once_the_clients_left() {
        // Arrange
        let (repository, recorded) = repository_with_client_session(1).await;
        let registry = Arc::new(StubRouteRegistry::with_migration_target(Some(
            "relay-b.example:4433",
        )));
        let drain = {
            let (repository, registry) = (repository.clone(), registry.clone());
            tokio::spawn(async move {
                drain_client_sessions(&repository, registry.as_ref(), DRAIN_TIMEOUT).await
            })
        };
        while recorded.go_aways().is_empty() {
            tokio::task::yield_now().await;
        }

        // Act: the client closes its session after the GOAWAY
        repository.lock().await.remove(1);
        tokio::time::timeout(Duration::from_secs(1), drain)
            .await
            .expect("drain must return once the last client session is gone")
            .unwrap();

        // Assert
        assert!(registry.is_draining());
        assert_eq!(recorded.go_aways(), vec!["moqt://relay-b.example:4433"]);
        assert!(recorded.closes().is_empty());
    }

    #[tokio::test]
    async fn drain_closes_the_clients_that_stay_past_the_timeout() {
        // Arrange
        let (repository, recorded) = repository_with_client_session(1).await;
        let registry = StubRouteRegistry::with_migration_target(None);

        // Act
        drain_client_sessions(&repository, &registry, Duration::from_millis(50)).await;

        // Assert: no migration target, so the GOAWAY names no URI
        assert_eq!(recorded.go_aways(), vec![""]);
        assert_eq!(
            recorded.closes(),
            vec![(
                TerminationErrorCode::GoawayTimeout,
                "the relay is shutting down".to_string()
            )]
        );
    }

    #[tokio::test]
    async fn relay_peers_receive_an_empty_go_away_and_are_not_waited_for() {
        // Arrange
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let repository = Arc::new(tokio::sync::Mutex::new(SessionRepository::new()));
        let (mut new_session, recorded) = mock_new_session(1, VerifiedToken::full_access());
        new_session.peer = SessionPeer::Relay;
        repository.lock().await.add(new_session, sender).await;
        let registry = StubRouteRegistry::with_migration_target(Some("relay-b.example:4433"));

        // Act
        let drained = tokio::time::timeout(
            Duration::from_secs(1),
            drain_client_sessions(&repository, &registry, DRAIN_TIMEOUT),
        )
        .await;

        // Assert
        assert!(drained.is_ok(), "a relay peer must not hold the drain");
        assert_eq!(recorded.go_aways(), vec![""]);
        assert!(recorded.closes().is_empty());
    }

    #[tokio::test]
    async fn drain_without_clients_returns_at_once() {
        // Arrange
        let repository = Arc::new(tokio::sync::Mutex::new(SessionRepository::new()));
        let registry = StubRouteRegistry::with_migration_target(None);

        // Act
        let drained = tokio::time::timeout(
            Duration::from_secs(1),
            drain_client_sessions(&repository, &registry, DRAIN_TIMEOUT),
        )
        .await;

        // Assert
        assert!(drained.is_ok());
    }
}
