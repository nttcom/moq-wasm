use moqt::wire::FetchParams;

use crate::modules::{
    auth::{
        authorize::{Denied, Operation, authorize},
        verified_token::VerifiedToken,
    },
    core::session_event::MoqtSessionEvent,
    enums::{
        FetchErrorCode, PublishErrorCode, PublishNamespaceErrorCode, SubscribeErrorCode,
        SubscribeNamespaceErrorCode,
    },
};

pub(crate) fn requested_access(event: &MoqtSessionEvent) -> Option<(Operation, Vec<String>)> {
    match event {
        MoqtSessionEvent::PublishNamespace(handler) => {
            Some((Operation::Publish, handler.track_namespace_tuple().to_vec()))
        }
        MoqtSessionEvent::Publish(handler) => {
            Some((Operation::Publish, handler.track_namespace_tuple().to_vec()))
        }
        MoqtSessionEvent::Subscribe(handler) => Some((
            Operation::Subscribe,
            handler.track_namespace_tuple().to_vec(),
        )),
        MoqtSessionEvent::SubscribeNamespace(handler) => Some((
            Operation::Subscribe,
            handler.track_namespace_prefix_tuple().to_vec(),
        )),
        MoqtSessionEvent::Fetch(handler) => match handler.fetch_params() {
            FetchParams::Standalone {
                track_namespace, ..
            } => Some((Operation::Subscribe, track_namespace)),
            FetchParams::RelativeJoining { .. } | FetchParams::AbsoluteJoining { .. } => None,
        },
        _ => None,
    }
}

pub(crate) fn authorize_request(
    token: Option<&VerifiedToken>,
    event: &MoqtSessionEvent,
) -> Result<(), Denied> {
    let Some((operation, namespace)) = requested_access(event) else {
        return Ok(());
    };
    let Some(token) = token else {
        return Err(Denied {
            reason: "session has no verified token",
        });
    };
    authorize(token, operation, &namespace)
}

pub(crate) async fn reject_unauthorized(event: MoqtSessionEvent, denied: Denied) {
    tracing::warn!(reason = denied.reason, "request denied");
    let reason = denied.reason.to_string();
    let sent = match event {
        MoqtSessionEvent::PublishNamespace(handler) => {
            handler
                .error(PublishNamespaceErrorCode::Unauthorized as u64, reason)
                .await
        }
        MoqtSessionEvent::Publish(handler) => {
            handler
                .error(PublishErrorCode::Unauthorized as u64, reason)
                .await
        }
        MoqtSessionEvent::Subscribe(handler) => {
            handler
                .error(SubscribeErrorCode::Unauthorized as u64, reason)
                .await
        }
        MoqtSessionEvent::SubscribeNamespace(handler) => {
            handler
                .error(SubscribeNamespaceErrorCode::Unauthorized as u64, reason)
                .await
        }
        MoqtSessionEvent::Fetch(handler) => {
            handler
                .error(FetchErrorCode::Unauthorized as u64, reason)
                .await
        }
        _ => unreachable!("only events with a requested_access are rejected"),
    };
    if let Err(error) = sent {
        tracing::warn!(%error, "failed to send UNAUTHORIZED error");
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use moqt::{FetchOption, Location, PublishOption, SubscribeOption, wire::RequestError};

    use crate::modules::auth::{
        test_support::{connect_client_with_token, spawn_relay_with_verifier},
        verified_token::{VerifiedToken, parse_namespace_path},
    };

    const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

    fn site1_token() -> VerifiedToken {
        VerifiedToken {
            app_id: "APP".to_string(),
            publish: Some(parse_namespace_path("site1")),
            subscribe: Some(parse_namespace_path("site1")),
            is_relay: false,
            expires_at: None,
        }
    }

    fn error_code(error: anyhow::Error) -> u64 {
        error
            .downcast_ref::<RequestError>()
            .unwrap_or_else(|| panic!("expected a request error, got {error:?}"))
            .error_code
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn publish_namespace_inside_the_granted_path_is_accepted() {
        // Arrange
        let relay = spawn_relay_with_verifier(site1_token()).await;
        let client = connect_client_with_token(relay.port, "jwt").await;

        // Act
        let result = tokio::time::timeout(
            REQUEST_TIMEOUT,
            client
                .publisher()
                .publish_namespace("APP/site1/cam1".to_string()),
        )
        .await
        .unwrap();

        // Assert
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn publish_namespace_outside_the_granted_path_is_unauthorized() {
        // Arrange
        let relay = spawn_relay_with_verifier(site1_token()).await;
        let client = connect_client_with_token(relay.port, "jwt").await;

        // Act
        let result = tokio::time::timeout(
            REQUEST_TIMEOUT,
            client
                .publisher()
                .publish_namespace("APP/site2".to_string()),
        )
        .await
        .unwrap();

        // Assert
        assert_eq!(error_code(result.unwrap_err()), 0x1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_namespace_inside_the_granted_path_is_accepted() {
        // Arrange
        let relay = spawn_relay_with_verifier(site1_token()).await;
        let client = connect_client_with_token(relay.port, "jwt").await;

        // Act
        let result = tokio::time::timeout(
            REQUEST_TIMEOUT,
            client
                .subscriber()
                .subscribe_namespace("APP/site1".to_string()),
        )
        .await
        .unwrap();

        // Assert
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_namespace_of_another_app_is_unauthorized() {
        // Arrange
        let relay = spawn_relay_with_verifier(site1_token()).await;
        let client = connect_client_with_token(relay.port, "jwt").await;

        // Act
        let result = tokio::time::timeout(
            REQUEST_TIMEOUT,
            client
                .subscriber()
                .subscribe_namespace("OTHER/site1".to_string()),
        )
        .await
        .unwrap();

        // Assert
        assert_eq!(error_code(result.unwrap_err()), 0x1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn publish_inside_the_granted_path_is_accepted() {
        // Arrange
        let relay = spawn_relay_with_verifier(site1_token()).await;
        let client = connect_client_with_token(relay.port, "jwt").await;

        // Act
        let result = tokio::time::timeout(
            REQUEST_TIMEOUT,
            client.publisher().publish(
                "APP/site1".to_string(),
                "video".to_string(),
                PublishOption::default(),
            ),
        )
        .await
        .unwrap();

        // Assert
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn publish_above_the_granted_path_is_unauthorized() {
        // Arrange
        let relay = spawn_relay_with_verifier(site1_token()).await;
        let client = connect_client_with_token(relay.port, "jwt").await;

        // Act
        let result = tokio::time::timeout(
            REQUEST_TIMEOUT,
            client.publisher().publish(
                "APP".to_string(),
                "video".to_string(),
                PublishOption::default(),
            ),
        )
        .await
        .unwrap();

        // Assert
        assert_eq!(error_code(result.unwrap_err()), 0x1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_outside_the_granted_path_is_unauthorized() {
        // Arrange
        let relay = spawn_relay_with_verifier(site1_token()).await;
        let client = connect_client_with_token(relay.port, "jwt").await;
        let mut subscriber = client.subscriber();

        // Act
        let result = tokio::time::timeout(
            REQUEST_TIMEOUT,
            subscriber.subscribe(
                "APP/site2/x".to_string(),
                "video".to_string(),
                SubscribeOption::default(),
            ),
        )
        .await
        .unwrap();

        // Assert
        assert_eq!(error_code(result.unwrap_err()), 0x1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn standalone_fetch_outside_the_granted_path_is_unauthorized() {
        // Arrange
        let relay = spawn_relay_with_verifier(site1_token()).await;
        let client = connect_client_with_token(relay.port, "jwt").await;
        let mut subscriber = client.subscriber();

        // Act
        let result = tokio::time::timeout(
            REQUEST_TIMEOUT,
            subscriber.fetch(
                "APP/site2/x".to_string(),
                "video".to_string(),
                Location {
                    group_id: 0,
                    object_id: 0,
                },
                Location {
                    group_id: 1,
                    object_id: 0,
                },
                FetchOption::default(),
            ),
        )
        .await
        .unwrap();

        // Assert
        assert_eq!(error_code(result.unwrap_err()), 0x1);
    }
}
