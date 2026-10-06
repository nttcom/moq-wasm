use super::WorkerDeps;
use crate::modules::{
    control_plane::sequences::{
        publish_namespace_done::PublishNamespaceDone, stop_ingress,
        unsubscribe_namespace::UnsubscribeNamespace,
    },
    domain::{pub_sub_directory::entry::RemovedSessionSubscriptions, session_id::SessionId},
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
        for released in removed_downstream.released_upstream_subscriptions {
            if released.publisher_session_id != removed_session_id
                && let Err(err) = forwarder
                    .unsubscribe(released.publisher_session_id, released.upstream_request_id)
                    .await
            {
                tracing::debug!(
                    ?err,
                    upstream_session_id = released.publisher_session_id,
                    request_id = released.upstream_request_id,
                    "failed to forward upstream unsubscribe during session cleanup"
                );
            }

            stop_ingress(
                &deps.ingress_sender,
                released.publisher_session_id,
                &removed_downstream.track_key,
            )
            .await;
        }
    }

    for track_key in removed.upstream_track_keys {
        stop_ingress(&deps.ingress_sender, removed_session_id, &track_key).await;
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

        for track_namespace in removed.publish_namespace_track_namespaces {
            PublishNamespaceDone::notify_local_subscribers(
                removed_session_id,
                &track_namespace,
                table,
                forwarder,
            )
            .await;
            PublishNamespaceDone::withdraw_namespace_publication(
                &track_namespace,
                forwarder,
                deps.route_registry.as_ref(),
                deps.inter_relay_connection_manager.as_ref(),
            )
            .await;
        }
    }
}
