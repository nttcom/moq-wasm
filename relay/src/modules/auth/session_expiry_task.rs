use std::{
    sync::Weak,
    time::{Duration, SystemTime},
};

use moqt::TerminationErrorCode;

use crate::modules::core::session::Session;

const EXPIRED_REASON: &str = "authorization token expired";

pub(crate) struct SessionExpiryTask {
    join_handle: tokio::task::JoinHandle<()>,
}

impl SessionExpiryTask {
    pub(crate) fn run(session: Weak<dyn Session>, expires_at: SystemTime) -> Self {
        let remaining = expires_at
            .duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO);
        let join_handle = tokio::spawn(async move {
            tokio::time::sleep(remaining).await;
            if let Some(session) = session.upgrade() {
                tracing::info!("authorization token expired; closing session");
                session.close(TerminationErrorCode::ExpiredAuthToken, EXPIRED_REASON);
            }
        });
        Self { join_handle }
    }
}

impl Drop for SessionExpiryTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::Arc,
        time::{Duration, SystemTime},
    };

    use moqt::TerminationErrorCode;

    use super::SessionExpiryTask;
    use crate::modules::core::mocks::mock_session;

    #[tokio::test(start_paused = true)]
    async fn closes_the_session_when_the_token_expires() {
        // Arrange
        let (session, recorded) = mock_session();
        let _task = SessionExpiryTask::run(
            Arc::downgrade(&session),
            SystemTime::now() + Duration::from_secs(60),
        );

        tokio::task::yield_now().await;

        // Act
        tokio::time::advance(Duration::from_secs(61)).await;
        tokio::task::yield_now().await;

        // Assert
        assert_eq!(
            recorded.closes(),
            vec![(
                TerminationErrorCode::ExpiredAuthToken,
                "authorization token expired".to_string()
            )]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_the_task_cancels_the_close() {
        // Arrange
        let (session, recorded) = mock_session();
        let task = SessionExpiryTask::run(
            Arc::downgrade(&session),
            SystemTime::now() + Duration::from_secs(60),
        );

        tokio::task::yield_now().await;

        // Act
        drop(task);
        tokio::time::advance(Duration::from_secs(61)).await;
        tokio::task::yield_now().await;

        // Assert
        assert!(recorded.closes().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn departed_session_is_ignored() {
        // Arrange
        let (session, recorded) = mock_session();
        let _task = SessionExpiryTask::run(
            Arc::downgrade(&session),
            SystemTime::now() + Duration::from_secs(60),
        );
        tokio::task::yield_now().await;
        drop(session);

        // Act
        tokio::time::advance(Duration::from_secs(61)).await;
        tokio::task::yield_now().await;

        // Assert
        assert!(recorded.closes().is_empty());
    }
}
