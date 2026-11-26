pub(crate) mod downstream_publish;
pub(crate) mod drain_release;
pub(crate) mod fetch;
pub(crate) mod go_away;
pub(crate) mod malformed_track;
pub(crate) mod new_group_request;
pub(crate) mod publish;
pub(crate) mod publish_namespace;
pub(crate) mod publish_namespace_done;
pub(crate) mod subscribe;
pub(crate) mod subscribe_namespace;
pub(crate) mod subscribe_update;
pub(crate) mod track_status;
pub(crate) mod unsubscribe;
pub(crate) mod unsubscribe_namespace;
pub(crate) mod upstream_forward;
pub(crate) mod upstream_publish_done;

use crate::modules::{
    cascading::{
        inter_relay_connection_manager::InterRelayConnectionManager,
        namespace_routes::NamespaceRoutes,
        route_registry::{RelayInfo, RelayRouteRegistry},
    },
    control_plane::control_message_forwarder::ControlMessageForwarder,
    data_plane::{
        cache::{cached_object::CacheStamp, store::TrackCacheStore},
        egress::{
            egress_task::{EgressStart, EgressTask},
            fetch_delivery::{EgressFetchRequest, FetchDeliveryTask},
        },
        ingress::ingest_task::{IngestStart, IngestTask},
    },
    directory::{
        InMemoryLocalPubSubDirectory,
        track::{Producer, ProducerOrigin, ReleasedUpstreamSubscription},
    },
    domain::{
        runner_signals::DownstreamRunnerSignals, session_id::SessionId, session_peer::SessionPeer,
        track_key::TrackKey,
    },
    session::subscription::{DownstreamSubscription, UpstreamSubscription},
};
use tokio::sync::oneshot;
use tracing::Span;

#[derive(Clone, Copy)]
pub(crate) struct CascadingRelayContext<'a> {
    pub(crate) route_registry: &'a dyn RelayRouteRegistry,
    pub(crate) namespace_routes: &'a NamespaceRoutes,
    pub(crate) inter_relay_connection_manager: &'a InterRelayConnectionManager,
}

/// Records `upstream_relay_id` and `upstream_request_id` on the current
/// span, which must declare both fields; the upstream relay records the same
/// pair as `peer_relay_id` and `request_id`. Records nothing when the
/// publisher is a client.
pub(crate) async fn record_upstream_relay(
    forwarder: &ControlMessageForwarder,
    publisher_session_id: SessionId,
    upstream_request_id: u64,
) {
    let relay_id = forwarder
        .repository
        .lock()
        .await
        .verified_token(publisher_session_id)
        .and_then(|token| token.relay_id.clone());
    if let Some(relay_id) = relay_id {
        let span = tracing::Span::current();
        span.record("upstream_relay_id", relay_id.as_str());
        span.record("upstream_request_id", upstream_request_id);
    }
}

pub(crate) async fn is_origin_client(
    session_id: SessionId,
    forwarder: &ControlMessageForwarder,
) -> bool {
    forwarder
        .repository
        .lock()
        .await
        .is_client_session(session_id)
}

pub(crate) async fn session_peer(
    session_id: SessionId,
    forwarder: &ControlMessageForwarder,
) -> SessionPeer {
    if is_origin_client(session_id, forwarder).await {
        SessionPeer::Client
    } else {
        SessionPeer::Relay
    }
}

pub(crate) async fn connect_relay(
    inter_relay_connection_manager: &InterRelayConnectionManager,
    relay: &RelayInfo,
) -> Option<SessionId> {
    inter_relay_connection_manager
        .get_or_connect(relay)
        .await
        .inspect_err(
            |err| tracing::warn!(error = ?err, relay_id = %relay.relay_id, "failed to connect relay"),
        )
        .ok()
}

/// Removes a downstream subscription its subscriber was never told of, and
/// releases the upstream subscriptions it leaves unneeded.
pub(crate) async fn remove_downstream_subscription(
    table: &InMemoryLocalPubSubDirectory,
    forwarder: &ControlMessageForwarder,
    namespace_routes: &NamespaceRoutes,
    subscriber_session_id: SessionId,
    request_id: u64,
) {
    let Some(removed) = table.remove_downstream_subscription(subscriber_session_id, request_id)
    else {
        return;
    };
    release_upstream_subscriptions(
        table,
        forwarder,
        namespace_routes,
        &removed.track_key,
        &removed.released_upstream_subscriptions,
    )
    .await;
}

pub(crate) async fn release_upstream_subscriptions(
    table: &InMemoryLocalPubSubDirectory,
    forwarder: &ControlMessageForwarder,
    namespace_routes: &NamespaceRoutes,
    track_key: &TrackKey,
    released: &[ReleasedUpstreamSubscription],
) {
    for released in released {
        release_upstream(
            forwarder,
            released.publisher_session_id,
            &released.subscription,
        )
        .await;
    }
    withdraw_released_track_route(table, namespace_routes, track_key, released).await;
}

/// Withdraws the route of `track_key` when the directory released a PUBLISH
/// of it and no client PUBLISHes it here any more: the directory forgot the
/// PUBLISH, so its PUBLISH_DONE finds no producer to withdraw the route with.
pub(crate) async fn withdraw_released_track_route(
    table: &InMemoryLocalPubSubDirectory,
    namespace_routes: &NamespaceRoutes,
    track_key: &TrackKey,
    released: &[ReleasedUpstreamSubscription],
) {
    if released
        .iter()
        .any(|released| released.origin == ProducerOrigin::Publish)
    {
        namespace_routes.withdraw_track(track_key, table).await;
    }
}

/// Sends the UNSUBSCRIBE of a released upstream subscription; its ingest
/// stopped with the producer the directory dropped.
pub(crate) async fn release_upstream(
    forwarder: &ControlMessageForwarder,
    publisher_session_id: SessionId,
    subscription: &UpstreamSubscription,
) {
    let upstream_request_id = subscription.request_id();
    if let Err(err) = forwarder
        .unsubscribe(publisher_session_id, subscription)
        .await
    {
        tracing::warn!(
            error = ?err,
            publisher_session_id = %publisher_session_id,
            request_id = %upstream_request_id,
            "failed to forward upstream unsubscribe"
        );
    } else {
        tracing::debug!(
            publisher_session_id = %publisher_session_id,
            request_id = %upstream_request_id,
            "forwarded upstream UNSUBSCRIBE"
        );
    }
}

/// Returns the Forward State for the PUBLISH_OK and unsubscribes the
/// publisher's earlier PUBLISH it replaces. `None` when this relay's
/// SUBSCRIBE already feeds the track: the caller answers the PUBLISH in
/// Forward State 0, then unsubscribes it.
pub(crate) async fn register_published_subscription(
    table: &InMemoryLocalPubSubDirectory,
    forwarder: &ControlMessageForwarder,
    track_key: TrackKey,
    publisher_session_id: SessionId,
    producer: Producer,
) -> Option<bool> {
    let (forward, replaced) =
        table.register_published_subscription(track_key, publisher_session_id, producer)?;
    let Some(replaced) = replaced else {
        return Some(forward);
    };
    if let Err(error) = forwarder
        .unsubscribe(publisher_session_id, &replaced.state.subscription)
        .await
    {
        tracing::warn!(
            ?error,
            publisher_session_id = %publisher_session_id,
            request_id = %replaced.state.subscription.request_id(),
            "failed to unsubscribe the replaced upstream subscription"
        );
    }
    Some(forward)
}

pub(crate) struct IngestRequest {
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) publisher_session_id: SessionId,
    pub(crate) track_key: TrackKey,
    pub(crate) subscription: UpstreamSubscription,
}

/// Starts reading the upstream subscription into the track's cache. `None`
/// when the publisher session already left the repository.
pub(crate) async fn start_ingest(
    forwarder: &ControlMessageForwarder,
    cache_store: &TrackCacheStore,
    request: IngestRequest,
) -> Option<IngestTask> {
    let IngestRequest {
        subscriber_session_id,
        publisher_session_id,
        track_key,
        subscription,
    } = request;
    let (subscriber, publisher_mailbox_sender, publisher_session_span, persist) = {
        let repository = forwarder.repository.lock().await;
        let Some((subscriber, publisher_mailbox_sender)) = repository
            .subscriber(publisher_session_id)
            .zip(repository.mailbox(publisher_session_id))
        else {
            tracing::warn!(track_namespace = %track_key.track_namespace, track_name = %track_key.track_name, publisher_session_id, "publisher session not found; ingest not started");
            return None;
        };
        (
            subscriber,
            publisher_mailbox_sender,
            repository
                .session_span(publisher_session_id)
                .unwrap_or_else(Span::none),
            // Only the relay a client publishes to keeps the payloads on
            // disk; downstream relays fetch from it.
            repository.is_client_session(publisher_session_id),
        )
    };
    // The data receiver tells whether the upstream compares the status of
    // duplicates; `read_track` sets it.
    let stamp = CacheStamp {
        max_cache_duration: subscription.max_cache_duration(),
        persist,
        status_may_change: false,
    };
    Some(IngestTask::run(
        IngestStart {
            cache: cache_store.get_or_create(&track_key),
            track_key,
            subscriber_session_id,
            publisher_session_id,
            subscriber,
            subscription,
            publisher_mailbox_sender,
            stamp,
            publisher_session_span,
        },
        &Span::current(),
    ))
}

pub(crate) struct EgressRequest {
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) request_id: u64,
    pub(crate) track_key: TrackKey,
    pub(crate) downstream_subscription: DownstreamSubscription,
    pub(crate) ready_sender: oneshot::Sender<anyhow::Result<()>>,
    pub(crate) runner_signals: DownstreamRunnerSignals,
    pub(crate) subscribe_ok_receiver: oneshot::Receiver<()>,
    pub(crate) largest_location: Option<moqt::object::Location>,
}

/// Starts the egress runner of a registered downstream subscription. `false`
/// when the subscriber session already left the repository; its cleanup may
/// have run before the registration, so the registration is removed and its
/// upstream subscriptions released here.
pub(crate) async fn start_egress(
    table: &InMemoryLocalPubSubDirectory,
    forwarder: &ControlMessageForwarder,
    namespace_routes: &NamespaceRoutes,
    cache_store: &TrackCacheStore,
    request: EgressRequest,
) -> bool {
    let (publisher, subscriber_mailbox_sender) = {
        let repository = forwarder.repository.lock().await;
        (
            repository.publisher(request.subscriber_session_id),
            repository.mailbox(request.subscriber_session_id),
        )
    };
    let (Some(publisher), Some(subscriber_mailbox_sender)) = (publisher, subscriber_mailbox_sender)
    else {
        tracing::error!(
            subscriber_session_id = request.subscriber_session_id,
            request_id = request.request_id,
            "subscriber session not found; egress not started"
        );
        remove_downstream_subscription(
            table,
            forwarder,
            namespace_routes,
            request.subscriber_session_id,
            request.request_id,
        )
        .await;
        return false;
    };
    let _egress = EgressTask::run(EgressStart {
        subscriber_session_id: request.subscriber_session_id,
        request_id: request.request_id,
        cache: cache_store.get_or_create(&request.track_key),
        track_key: request.track_key,
        downstream_subscription: request.downstream_subscription,
        publisher,
        subscriber_mailbox_sender,
        parent_span: Span::current(),
        ready_sender: request.ready_sender,
        runner_signals: request.runner_signals,
        subscribe_ok_receiver: request.subscribe_ok_receiver,
        largest_location: request.largest_location,
    });
    true
}

/// Serves a FETCH from the cache on the subscriber's session. `false` when
/// the subscriber session already left the repository.
pub(crate) async fn start_fetch_delivery(
    forwarder: &ControlMessageForwarder,
    request: EgressFetchRequest,
) -> bool {
    let publisher = forwarder
        .repository
        .lock()
        .await
        .publisher(request.subscriber_session_id);
    let Some(publisher) = publisher else {
        tracing::error!(
            subscriber_session_id = request.subscriber_session_id,
            request_id = request.request_id,
            "subscriber session not found; fetch not delivered"
        );
        return false;
    };
    let _delivery = FetchDeliveryTask::run(publisher, request);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::test_support::directory_fixtures::downstream_subscription;
    use crate::modules::{
        directory::track::{ProducerOrigin, ProducerState},
        test_support::{
            directory_fixtures::{
                PUBLISHER_SESSION, UPSTREAM_REQUEST_ID, active_upstream, delivery_control,
                producer_with, register_downstream, track_key, upstream_release_context,
            },
            mock_session::wait_until,
            relay_harness::fixtures::subscription::upstream_subscription,
        },
    };

    const PUBLISH_REQUEST_ID: u64 = 43;

    fn published() -> Producer {
        producer_with(ProducerState {
            subscription: upstream_subscription(PUBLISH_REQUEST_ID),
            ..active_upstream(ProducerOrigin::Publish)
        })
    }

    #[tokio::test]
    async fn a_publish_of_a_subscribed_track_keeps_the_subscribe_as_published() {
        // Arrange
        let ctx = upstream_release_context(ProducerOrigin::Subscribe).await;

        // Act
        let forward = register_published_subscription(
            &ctx.table,
            &ctx.forwarder,
            track_key(),
            PUBLISHER_SESSION,
            published(),
        )
        .await;

        // Assert: nothing is unsubscribed or stopped, and the track counts as the client's published track
        assert_eq!(forward, None);
        assert!(ctx.recorded.unsubscribed_request_ids().is_empty());
        assert!(
            !ctx.recorded
                .ingests_stopped()
                .contains(&UPSTREAM_REQUEST_ID)
        );
        assert!(ctx.table.has_upstream_subscription(
            &track_key(),
            PUBLISHER_SESSION,
            UPSTREAM_REQUEST_ID
        ));
        assert!(ctx.table.has_client_published_track(&track_key()));
    }

    #[tokio::test]
    async fn a_second_publish_of_a_track_unsubscribes_the_first() {
        // Arrange
        let ctx = upstream_release_context(ProducerOrigin::Publish).await;

        // Act
        let forward = register_published_subscription(
            &ctx.table,
            &ctx.forwarder,
            track_key(),
            PUBLISHER_SESSION,
            published(),
        )
        .await;

        // Assert
        assert_eq!(forward, Some(true));
        assert_eq!(
            ctx.recorded.unsubscribed_request_ids(),
            vec![UPSTREAM_REQUEST_ID]
        );
        ctx.recorded.wait_ingest_stopped(UPSTREAM_REQUEST_ID).await;
    }

    #[tokio::test]
    async fn the_last_subscriber_leaving_a_draining_relay_releases_the_kept_subscribe() {
        // Arrange
        let ctx = upstream_release_context(ProducerOrigin::Subscribe).await;
        register_downstream(
            &ctx.table,
            100,
            10,
            SessionPeer::Client,
            downstream_subscription(track_key(), None),
            delivery_control(true),
        )
        .unwrap();
        register_published_subscription(
            &ctx.table,
            &ctx.forwarder,
            track_key(),
            PUBLISHER_SESSION,
            published(),
        )
        .await;
        ctx.table.begin_draining(0);

        // Act
        let removed = ctx.table.remove_downstream_subscription(100, 10).unwrap();

        // Assert: the track leaves no route owner
        let released: Vec<_> = removed
            .released_upstream_subscriptions
            .iter()
            .map(|released| released.subscription.request_id())
            .collect();
        assert_eq!(released, vec![UPSTREAM_REQUEST_ID]);
        assert!(ctx.table.upstream_tracks.get(&track_key()).is_none());
        assert!(!ctx.table.has_client_published_track(&track_key()));
    }

    #[tokio::test]
    async fn a_publish_racing_the_last_subscriber_of_a_draining_relay_leaves_no_route_owner() {
        // Arrange: the PUBLISH replaces the session's earlier PUBLISH, whose UNSUBSCRIBE it awaits
        let ctx = upstream_release_context(ProducerOrigin::Publish).await;
        register_downstream(
            &ctx.table,
            100,
            10,
            SessionPeer::Client,
            downstream_subscription(track_key(), None),
            delivery_control(true),
        )
        .unwrap();
        ctx.table.begin_draining(0);
        let unsubscribe_gate = ctx.recorded.hold_unsubscribes();

        // Act: the last subscriber leaves while the PUBLISH awaits the UNSUBSCRIBE
        tokio::join!(
            register_published_subscription(
                &ctx.table,
                &ctx.forwarder,
                track_key(),
                PUBLISHER_SESSION,
                published(),
            ),
            async {
                wait_until(|| ctx.recorded.unsubscribed_request_ids() == [UPSTREAM_REQUEST_ID])
                    .await
                    .unwrap();
                ctx.table.remove_downstream_subscription(100, 10).unwrap();
                unsubscribe_gate.notify_one();
            },
        );

        // Assert
        assert!(ctx.table.upstream_tracks.get(&track_key()).is_none());
        assert!(!ctx.table.has_client_published_track(&track_key()));
    }
}
