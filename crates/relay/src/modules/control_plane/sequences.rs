pub(crate) mod downstream_publish;
pub(crate) mod drain_release;
pub(crate) mod fetch;
pub(crate) mod go_away;
pub(crate) mod malformed_track;
pub(crate) mod publish;
pub(crate) mod publish_namespace;
pub(crate) mod publish_namespace_done;
pub(crate) mod subscribe;
pub(crate) mod subscribe_namespace;
pub(crate) mod subscribe_update;
pub(crate) mod track_status;
pub(crate) mod unsubscribe;
pub(crate) mod unsubscribe_namespace;
pub(crate) mod upstream_publish_done;

use crate::modules::{
    cascading::{
        inter_relay_connection_manager::InterRelayConnectionManager,
        route_registry::{RelayInfo, RelayRouteRegistry},
    },
    control_plane::control_message_forwarder::ControlMessageForwarder,
    data_plane::{
        cache::{cached_object::CacheStamp, store::TrackCacheStore},
        ingress::ingest_task::{IngestStart, IngestTask},
    },
    directory::{InMemoryLocalPubSubDirectory, track::Producer},
    domain::{session_id::SessionId, session_peer::SessionPeer, track_key::TrackKey},
    session::subscription::UpstreamSubscription,
};
use tracing::Span;

#[derive(Clone, Copy)]
pub(crate) struct CascadingRelayContext<'a> {
    pub(crate) route_registry: &'a dyn RelayRouteRegistry,
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

/// Also unsubscribes the subscription from the same publisher that this one
/// replaces, e.g. the SUBSCRIBE this relay sent before the publisher sent
/// PUBLISH for the track; the replaced producer's ingest stops with it.
pub(crate) async fn register_upstream_subscription(
    table: &InMemoryLocalPubSubDirectory,
    forwarder: &ControlMessageForwarder,
    track_key: TrackKey,
    publisher_session_id: SessionId,
    producer: Producer,
) {
    let Some(replaced) =
        table.register_upstream_subscription(track_key, publisher_session_id, producer)
    else {
        return;
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
    let stamp = CacheStamp {
        max_cache_duration: subscription.max_cache_duration(),
        persist,
    };
    Some(IngestTask::run(IngestStart {
        cache: cache_store.get_or_create(&track_key),
        track_key,
        subscriber_session_id,
        publisher_session_id,
        subscriber,
        subscription,
        publisher_mailbox_sender,
        stamp,
        request_span: Span::current(),
        publisher_session_span,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{
        directory::track::{ProducerOrigin, ProducerState},
        test_support::{
            directory_fixtures::{
                PUBLISHER_SESSION, UPSTREAM_REQUEST_ID, active_upstream, delivery_control,
                producer_with, track_key, upstream_release_context,
            },
            relay_harness::fixtures::subscription::upstream_subscription,
        },
    };
    use moqt::fetch::JoinPoint;

    const PUBLISH_REQUEST_ID: u64 = 43;

    fn published() -> Producer {
        producer_with(ProducerState {
            subscription: upstream_subscription(PUBLISH_REQUEST_ID),
            ..active_upstream(ProducerOrigin::Publish)
        })
    }

    #[tokio::test]
    async fn a_publish_of_a_subscribed_track_unsubscribes_the_subscribe() {
        // Arrange
        let ctx = upstream_release_context(ProducerOrigin::Subscribe).await;

        // Act
        register_upstream_subscription(
            &ctx.table,
            &ctx.forwarder,
            track_key(),
            PUBLISHER_SESSION,
            published(),
        )
        .await;

        // Assert: the SUBSCRIBE the relay sent first is unsubscribed, and its ingest stops with it
        assert_eq!(
            ctx.recorded.unsubscribed_request_ids(),
            vec![UPSTREAM_REQUEST_ID]
        );
        ctx.recorded.wait_ingest_stopped(UPSTREAM_REQUEST_ID).await;
    }

    #[tokio::test]
    async fn the_last_subscriber_leaving_a_draining_relay_leaves_the_publisher_unsubscribed() {
        // Arrange
        let ctx = upstream_release_context(ProducerOrigin::Subscribe).await;
        ctx.table
            .register_downstream_subscription(
                100,
                10,
                SessionPeer::Client,
                track_key(),
                JoinPoint::Largest(None),
                delivery_control(true),
            )
            .unwrap();
        register_upstream_subscription(
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
        for released in removed.released_upstream_subscriptions {
            ctx.forwarder
                .unsubscribe(released.publisher_session_id, &released.subscription)
                .await
                .unwrap();
        }

        // Assert: both the SUBSCRIBE the relay sent first and the PUBLISH are unsubscribed
        assert_eq!(
            ctx.recorded.unsubscribed_request_ids(),
            vec![UPSTREAM_REQUEST_ID, PUBLISH_REQUEST_ID]
        );
    }
}
