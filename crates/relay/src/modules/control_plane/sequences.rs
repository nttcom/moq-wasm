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
    data_plane::ingress::ingress_coordinator::{IngressCommand, IngressStartRequest},
    domain::{
        pub_sub_directory::{InMemoryLocalPubSubDirectory, entry::ActiveUpstreamSubscription},
        session_id::SessionId,
        session_peer::SessionPeer,
        track_key::TrackKey,
    },
    session::subscription::UpstreamSubscription,
};

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

pub(crate) async fn release_upstream(
    forwarder: &ControlMessageForwarder,
    ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
    publisher_session_id: SessionId,
    subscription: &UpstreamSubscription,
    track_key: &TrackKey,
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

    stop_ingress(ingress_sender, publisher_session_id, track_key).await;
}

/// Unsubscribes the subscription from the same publisher that this one
/// replaces, e.g. the SUBSCRIBE this relay sent before the publisher sent
/// PUBLISH for the track: the publisher would otherwise keep serving it.
/// The caller has already started this subscription's ingress, which stopped
/// the replaced one's.
pub(crate) async fn register_upstream_subscription(
    table: &InMemoryLocalPubSubDirectory,
    forwarder: &ControlMessageForwarder,
    track_key: TrackKey,
    publisher_session_id: SessionId,
    active_upstream: ActiveUpstreamSubscription,
) {
    let Some(replaced) =
        table.register_upstream_subscription(track_key, publisher_session_id, active_upstream)
    else {
        return;
    };
    if let Err(error) = forwarder
        .unsubscribe(publisher_session_id, &replaced.subscription)
        .await
    {
        tracing::warn!(
            ?error,
            publisher_session_id = %publisher_session_id,
            request_id = %replaced.subscription.request_id(),
            "failed to unsubscribe the replaced upstream subscription"
        );
    }
}

pub(crate) async fn start_ingress(
    ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
    request: IngressStartRequest,
) -> bool {
    let track_key = request.track_key.clone();
    let publisher_session_id = request.publisher_session_id;
    let started = ingress_sender
        .send(IngressCommand::Start(Box::new(request)))
        .await
        .is_ok();
    if !started {
        tracing::error!(track_namespace = %track_key.track_namespace, track_name = %track_key.track_name, publisher_session_id, "failed to send ingress start request");
    }
    started
}

pub(crate) async fn stop_ingress(
    ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
    publisher_session_id: SessionId,
    track_key: &TrackKey,
) {
    if ingress_sender
        .send(IngressCommand::StopTrack {
            track_key: track_key.clone(),
            publisher_session_id,
        })
        .await
        .is_err()
    {
        tracing::error!(track_namespace = %track_key.track_namespace, track_name = %track_key.track_name, "failed to send ingress stop request");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{
        domain::pub_sub_directory::entry::UpstreamSubscriptionOrigin,
        test_support::{
            directory_fixtures::{
                PUBLISHER_SESSION, UPSTREAM_REQUEST_ID, active_upstream, delivery_control,
                track_key, upstream_release_context,
            },
            relay_harness::fixtures::subscription::upstream_subscription,
        },
    };
    use moqt::fetch::JoinPoint;

    const PUBLISH_REQUEST_ID: u64 = 43;

    fn published() -> ActiveUpstreamSubscription {
        ActiveUpstreamSubscription {
            subscription: upstream_subscription(PUBLISH_REQUEST_ID),
            ..active_upstream(UpstreamSubscriptionOrigin::Publish)
        }
    }

    #[tokio::test]
    async fn a_publish_of_a_subscribed_track_unsubscribes_the_subscribe() {
        // Arrange
        let mut ctx = upstream_release_context(UpstreamSubscriptionOrigin::Subscribe).await;

        // Act
        register_upstream_subscription(
            &ctx.table,
            &ctx.forwarder,
            track_key(),
            PUBLISHER_SESSION,
            published(),
        )
        .await;

        // Assert: the PUBLISH's ingress already replaced the SUBSCRIBE's, so none is stopped
        assert_eq!(
            ctx.recorded.unsubscribed_request_ids(),
            vec![UPSTREAM_REQUEST_ID]
        );
        assert!(ctx.ingress_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn the_last_subscriber_leaving_a_draining_relay_leaves_the_publisher_unsubscribed() {
        // Arrange
        let ctx = upstream_release_context(UpstreamSubscriptionOrigin::Subscribe).await;
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

        // Assert: neither the SUBSCRIBE the relay sent first nor the PUBLISH is left
        assert_eq!(
            ctx.recorded.unsubscribed_request_ids(),
            vec![UPSTREAM_REQUEST_ID, PUBLISH_REQUEST_ID]
        );
    }
}
