use super::WorkerDeps;
use crate::modules::{
    control_plane::sequences::{
        publish::Publish, publish_namespace_done::PublishNamespaceDone,
        unsubscribe_namespace::UnsubscribeNamespace, withdraw_released_track_route,
    },
    directory::track::RemovedSessionSubscriptions,
    domain::session_id::SessionId,
};

/// Idempotent: safe to call when the session is already absent. The session
/// leaves the repository before the directory, so a publisher joining a track
/// concurrently either is found by `remove_session` or finds its session gone.
pub(super) async fn cleanup_session(session_id: SessionId, deps: &WorkerDeps) {
    let was_client = {
        let mut repository = deps.control_message_forwarder.repository.lock().await;
        let was_client = repository.is_client_session(session_id);
        repository.remove(session_id);
        was_client
    };
    let removed = deps.local_pub_sub_directory.remove_session(session_id);
    deps.cache_store.forget_downstream_session(session_id);
    deps.inter_relay_connection_manager
        .namespace_subscriptions
        .forget_session(session_id);
    cleanup_removed_session(session_id, was_client, removed, deps).await;
}

async fn cleanup_removed_session(
    removed_session_id: SessionId,
    was_client: bool,
    removed: RemovedSessionSubscriptions,
    deps: &WorkerDeps,
) {
    let table = deps.local_pub_sub_directory.as_ref();
    let forwarder = &deps.control_message_forwarder;

    for removed_downstream in removed.downstream_subscriptions {
        withdraw_released_track_route(
            table,
            deps.route_registry.as_ref(),
            &removed_downstream.track_key,
            &removed_downstream.released_upstream_subscriptions,
        )
        .await;
        for released in removed_downstream.released_upstream_subscriptions {
            if released.publisher_session_id != removed_session_id
                && let Err(err) = forwarder
                    .unsubscribe(released.publisher_session_id, &released.subscription)
                    .await
            {
                tracing::debug!(
                    error = ?err,
                    publisher_session_id = released.publisher_session_id,
                    request_id = released.subscription.request_id(),
                    "failed to forward upstream unsubscribe during session cleanup"
                );
            }
        }
    }

    for track_key in removed.upstream_track_keys {
        if !table.has_track_publisher(&track_key) {
            deps.cache_store.publisher_left(&track_key);
        }
        if was_client {
            Publish::withdraw_track_route(table, deps.route_registry.as_ref(), &track_key).await;
        }
    }

    if was_client {
        for track_namespace_prefix in removed.subscribe_namespace_prefixes {
            UnsubscribeNamespace::cleanup_empty_namespace_subscription(
                &track_namespace_prefix,
                table,
                forwarder,
                deps.route_registry.as_ref(),
                deps.inter_relay_connection_manager.as_ref(),
            )
            .await;
        }
    }

    for track_namespace in removed.publish_namespace_track_namespaces {
        PublishNamespaceDone::notify_local_subscribers(
            removed_session_id,
            was_client,
            &track_namespace,
            table,
            forwarder,
        )
        .await;
        if was_client {
            PublishNamespaceDone::withdraw_namespace_publication(
                &track_namespace,
                table,
                forwarder,
                deps.cascading_relay_context(),
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use moqt::{QUIC, SessionEvent, TerminationErrorCode, Version};

    use crate::modules::{
        auth::test_support::{
            app_token, client_endpoint, connect, connect_client, relay_server_offering,
            relay_token, spawn_relay,
        },
        cascading::route_registry::NoopRelayRouteRegistry,
        test_support::{
            relay_clients::{next_event, publish_namespace, subscribe_namespace},
            stub_route_registry::StubRouteRegistry,
            within,
        },
    };

    #[tokio::test]
    async fn a_client_is_told_when_the_relay_publishing_its_namespace_disconnects() {
        for version in [Version::Draft14, Version::Draft18] {
            // Arrange
            let relay =
                relay_server_offering(relay_token(), Arc::new(NoopRelayRouteRegistry), &[version]);
            let outer = relay.server.spawn_client_transport::<QUIC>(0).unwrap();
            let inner = relay.server.spawn_inner_transport::<QUIC>(0).unwrap();
            let watcher = connect(outer.local_port, client_endpoint(None)).await;
            let publisher =
                connect(inner.local_port, client_endpoint(Some("relay".to_string()))).await;
            subscribe_namespace(&watcher, "anon/room").await;
            publish_namespace(&publisher, "anon/room/camera").await;
            let SessionEvent::PublishNamespace(announced) = next_event(&watcher).await else {
                panic!("expected the camera namespace");
            };
            announced.ok().await.unwrap();

            // Act
            publisher.close_with_error(TerminationErrorCode::NoError, "leaving");

            // Assert
            let SessionEvent::PublishNamespaceDone(done) = next_event(&watcher).await else {
                panic!("expected the camera namespace to be withdrawn");
            };
            assert_eq!(done.track_namespace, "anon/room/camera");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disconnect_of_the_last_namespace_subscriber_removes_its_route() {
        // Arrange
        let route_registry = Arc::new(StubRouteRegistry::with_namespace_routes(&[]));
        let relay = spawn_relay(app_token(Some(""), Some("")), route_registry.clone()).await;
        let watcher = connect_client(&relay).await;
        subscribe_namespace(&watcher, "APP/room").await;

        // Act
        watcher.close_with_error(TerminationErrorCode::NoError, "leaving");

        // Assert
        within(async {
            while !route_registry
                .namespace_subscriber_calls()
                .contains(&"unregister APP/room".to_string())
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
    }
}
