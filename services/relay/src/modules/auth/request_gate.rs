use moqt::RequestErrorCode;
use moqt::fetch::FetchParams;

use crate::modules::{
    auth::{
        authorize::{Denied, Operation, authorize},
        verified_token::VerifiedToken,
    },
    session::moqt_session_event::MoqtSessionEvent,
};

pub(crate) fn requested_access(event: &MoqtSessionEvent) -> Option<(Operation, Vec<String>)> {
    match event {
        MoqtSessionEvent::PublishNamespace(handler) => {
            Some((Operation::Publish, handler.track_namespace_tuple().to_vec()))
        }
        MoqtSessionEvent::PublishNamespaceDone(handler) => {
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
        MoqtSessionEvent::SubscribeNamespaceUpdate(handler) => Some((
            Operation::Subscribe,
            handler.new_track_namespace_prefix_tuple().to_vec(),
        )),
        MoqtSessionEvent::Fetch(handler) => match handler.fetch_params() {
            FetchParams::Standalone {
                track_namespace, ..
            } => Some((Operation::Subscribe, track_namespace)),
            FetchParams::RelativeJoining { .. } | FetchParams::AbsoluteJoining { .. } => None,
        },
        MoqtSessionEvent::TrackStatus(handler) if handler.authorization_tokens().is_empty() => {
            Some((
                Operation::Subscribe,
                handler.track_namespace_tuple().to_vec(),
            ))
        }
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
    if let MoqtSessionEvent::PublishNamespaceDone(handler) = &event {
        // PUBLISH_NAMESPACE_DONE has no error response (draft-14 §9.26).
        tracing::warn!(
            reason = denied.reason,
            track_namespace = handler.track_namespace(),
            "PUBLISH_NAMESPACE_DONE denied; dropped"
        );
        return;
    }
    tracing::warn!(reason = denied.reason, "request denied");
    let reason = denied.reason.to_string();
    let sent = match event {
        MoqtSessionEvent::PublishNamespace(handler) => {
            handler.error(RequestErrorCode::Unauthorized, reason).await
        }
        MoqtSessionEvent::Publish(handler) => {
            handler.error(RequestErrorCode::Unauthorized, reason).await
        }
        MoqtSessionEvent::Subscribe(handler) => {
            handler.error(RequestErrorCode::Unauthorized, reason).await
        }
        MoqtSessionEvent::SubscribeNamespace(handler) => {
            handler.error(RequestErrorCode::Unauthorized, reason).await
        }
        MoqtSessionEvent::SubscribeNamespaceUpdate(handler) => {
            handler.error(RequestErrorCode::Unauthorized, reason).await;
            Ok(())
        }
        MoqtSessionEvent::Fetch(handler) => {
            handler.error(RequestErrorCode::Unauthorized, reason).await
        }
        MoqtSessionEvent::TrackStatus(handler) => {
            handler.error(RequestErrorCode::Unauthorized, reason).await
        }
        _ => unreachable!("only events with a requested_access are rejected"),
    };
    if let Err(error) = sent {
        tracing::warn!(%error, "failed to send UNAUTHORIZED error");
    }
}

#[cfg(test)]
mod tests {
    use moqt::RequestErrorCode;
    use std::time::Duration;

    use moqt::{
        fetch::FetchOption, namespace::NamespaceInterest, object::Location, publish::PublishOption,
        subscribe::SubscribeOption,
    };

    use moqt::namespace::PublishNamespaceDoneHandler;

    use super::{authorize_request, reject_unauthorized};
    use crate::modules::{
        auth::{
            test_support::{app_token, request_error_code, spawn_relay_and_connect_client},
            verified_token::VerifiedToken,
        },
        session::moqt_session_event::MoqtSessionEvent,
        test_support::mock_session::MockSubscribeNamespaceUpdateHandler,
    };

    const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

    fn site1_token() -> VerifiedToken {
        app_token(Some("site1"), Some("site1"))
    }

    fn publish_namespace_done(track_namespace: &str) -> MoqtSessionEvent {
        MoqtSessionEvent::PublishNamespaceDone(PublishNamespaceDoneHandler {
            track_namespace: track_namespace.to_string(),
            track_namespace_tuple: track_namespace.split('/').map(str::to_string).collect(),
        })
    }

    #[test]
    fn publish_namespace_done_inside_the_granted_path_is_authorized() {
        // Arrange
        let token = site1_token();

        // Act
        let result = authorize_request(Some(&token), &publish_namespace_done("APP/site1/cam1"));

        // Assert
        assert!(result.is_ok());
    }

    #[test]
    fn publish_namespace_done_outside_the_granted_path_is_denied() {
        // Arrange
        let token = site1_token();

        // Act
        let result = authorize_request(Some(&token), &publish_namespace_done("APP/site2"));

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn publish_namespace_done_without_a_token_is_denied() {
        // Act
        let result = authorize_request(None, &publish_namespace_done("APP/site1"));

        // Assert
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn denied_publish_namespace_done_is_dropped_without_a_response() {
        // Arrange
        let event = publish_namespace_done("APP/site2");
        let denied = authorize_request(Some(&site1_token()), &event).unwrap_err();

        // Act / Assert: returns instead of reaching the unreachable arm
        reject_unauthorized(event, denied).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn publish_namespace_inside_the_granted_path_is_accepted() {
        // Arrange
        let (_relay, client) = spawn_relay_and_connect_client(site1_token()).await;

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
        let (_relay, client) = spawn_relay_and_connect_client(site1_token()).await;

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
        assert_eq!(
            request_error_code(result.unwrap_err()),
            RequestErrorCode::Unauthorized
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_namespace_inside_the_granted_path_is_accepted() {
        // Arrange
        let (_relay, client) = spawn_relay_and_connect_client(site1_token()).await;

        // Act
        let result = tokio::time::timeout(
            REQUEST_TIMEOUT,
            client.subscriber().subscribe_namespace(
                "APP/site1".to_string(),
                NamespaceInterest::NamespacesAndTracks,
            ),
        )
        .await
        .unwrap();

        // Assert
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_namespace_of_another_app_is_unauthorized() {
        // Arrange
        let (_relay, client) = spawn_relay_and_connect_client(site1_token()).await;

        // Act
        let result = tokio::time::timeout(
            REQUEST_TIMEOUT,
            client.subscriber().subscribe_namespace(
                "OTHER/site1".to_string(),
                NamespaceInterest::NamespacesAndTracks,
            ),
        )
        .await
        .unwrap();

        // Assert
        assert_eq!(
            request_error_code(result.unwrap_err()),
            RequestErrorCode::Unauthorized
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn publish_inside_the_granted_path_is_accepted() {
        // Arrange
        let (_relay, client) = spawn_relay_and_connect_client(site1_token()).await;

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
        let (_relay, client) = spawn_relay_and_connect_client(site1_token()).await;

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
        assert_eq!(
            request_error_code(result.unwrap_err()),
            RequestErrorCode::Unauthorized
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_outside_the_granted_path_is_unauthorized() {
        // Arrange
        let (_relay, client) = spawn_relay_and_connect_client(site1_token()).await;
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
        assert_eq!(
            request_error_code(result.unwrap_err()),
            RequestErrorCode::Unauthorized
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn track_status_query_inside_the_granted_path_reaches_the_relay() {
        // Arrange
        let (_relay, client) = spawn_relay_and_connect_client(site1_token()).await;

        // Act
        let result = tokio::time::timeout(
            REQUEST_TIMEOUT,
            client.subscriber().track_status(
                "APP/site1/x".to_string(),
                "video".to_string(),
                vec![],
            ),
        )
        .await
        .unwrap();

        // Assert: NOT_SUPPORTED, because nothing subscribes to the track yet
        assert_eq!(
            request_error_code(result.unwrap_err()),
            RequestErrorCode::NotSupported
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn track_status_query_outside_the_granted_path_is_unauthorized() {
        // Arrange
        let (_relay, client) = spawn_relay_and_connect_client(site1_token()).await;

        // Act
        let result = tokio::time::timeout(
            REQUEST_TIMEOUT,
            client.subscriber().track_status(
                "APP/site2/x".to_string(),
                "video".to_string(),
                vec![],
            ),
        )
        .await
        .unwrap();

        // Assert
        assert_eq!(
            request_error_code(result.unwrap_err()),
            RequestErrorCode::Unauthorized
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn standalone_fetch_outside_the_granted_path_is_unauthorized() {
        // Arrange
        let (_relay, client) = spawn_relay_and_connect_client(site1_token()).await;
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
        assert_eq!(
            request_error_code(result.unwrap_err()),
            RequestErrorCode::Unauthorized
        );
    }

    #[test]
    fn a_namespace_prefix_update_needs_access_to_the_new_prefix() {
        // Arrange
        let update = |new_prefix| {
            MoqtSessionEvent::SubscribeNamespaceUpdate(Box::new(
                MockSubscribeNamespaceUpdateHandler::new(
                    "APP/site1",
                    new_prefix,
                    NamespaceInterest::Namespaces,
                ),
            ))
        };

        // Act
        let inside = authorize_request(Some(&site1_token()), &update("APP/site1/room"));
        let outside = authorize_request(Some(&site1_token()), &update("OTHER/site1"));

        // Assert: draft-18 §10.18 authorizes the namespace subscription it becomes
        assert!(inside.is_ok(), "{inside:?}");
        assert!(outside.is_err());
    }
}
