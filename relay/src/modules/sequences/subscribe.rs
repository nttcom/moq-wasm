use std::sync::Arc;

use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    core::handler::subscribe::SubscribeHandler,
    enums::{ContentExists, Location},
    relay::{
        cache::store::TrackCacheStore,
        egress::coordinator::{EgressCommand, EgressStartRequest},
        ingress::ingress_coordinator::{IngressCommand, IngressStartRequest},
    },
    sequences::tables::table::{
        ActiveUpstreamSubscription, LocalPubSubDirectory, UpstreamSubscriptionKey,
        UpstreamSubscriptionOrigin,
    },
    types::{SessionId, compose_session_track_key},
    upstream_publisher_resolver::UpstreamPublisherResolver,
};
use tracing::Span;

pub(crate) struct Subscribe;

/// Where the subscribe-time Largest Object comes from: the upstream
/// SUBSCRIBE_OK, or the local cache when joining an active upstream.
enum LargestObjectSource {
    LocalCache,
    SubscribeOk(Option<moqt::Location>),
}

enum UpstreamSubscriptionError {
    PublisherNotFound,
    SubscribeFailed,
    IngressStartFailed,
}

impl UpstreamSubscriptionError {
    fn code(&self) -> u64 {
        0
    }

    fn reason_phrase(&self) -> String {
        match self {
            Self::PublisherNotFound => "Designated namespace and track name do not exist.",
            Self::SubscribeFailed => "Failed to create upstream subscription.",
            Self::IngressStartFailed => "Failed to start upstream ingress.",
        }
        .to_string()
    }
}

impl Subscribe {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe",
        skip_all,
        parent = session_span,
        fields(session_id = %session_id)
    )]
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &dyn LocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        upstream_publisher_resolver: &UpstreamPublisherResolver,
        cache_store: &Arc<TrackCacheStore>,
        handler: Box<dyn SubscribeHandler>,
    ) {
        let track_namespace = handler.track_namespace();
        let track_name = handler.track_name();
        tracing::info!(
            session_id = %session_id,
            track_namespace = %track_namespace,
            track_name = %track_name,
            "SequenceHandler::subscribe"
        );

        let (upstream_key, active_upstream, largest_source) = match self
            .get_or_create_upstream_subscription(
                session_id,
                track_namespace,
                track_name,
                table,
                forwarder,
                ingress_sender,
                upstream_publisher_resolver,
            )
            .await
        {
            Ok(upstream_subscription) => upstream_subscription,
            Err(err) => {
                tracing::warn!(
                    session_id = %session_id,
                    track_namespace = %track_namespace,
                    track_name = %track_name,
                    "failed to get or create upstream subscription"
                );
                if let Err(send_error) = self
                    .response_error(handler.as_ref(), err.code(), err.reason_phrase())
                    .await
                {
                    tracing::error!(
                        subscribe_id = handler.subscribe_id(),
                        track_namespace = %track_namespace,
                        track_name = %track_name,
                        error = ?send_error,
                        "failed to send SUBSCRIBE_ERROR"
                    );
                }
                return;
            }
        };

        self.accept_downstream_subscription(
            session_id,
            upstream_key,
            active_upstream,
            largest_source,
            table,
            egress_sender,
            cache_store,
            handler.as_ref(),
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe.get_or_create_upstream_subscription",
        skip_all,
        fields(
            session_id = %session_id,
            track_namespace = %track_namespace,
            track_name = %track_name
        )
    )]
    async fn get_or_create_upstream_subscription(
        &self,
        session_id: SessionId,
        track_namespace: &str,
        track_name: &str,
        table: &dyn LocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
        upstream_publisher_resolver: &UpstreamPublisherResolver,
    ) -> Result<
        (
            UpstreamSubscriptionKey,
            ActiveUpstreamSubscription,
            LargestObjectSource,
        ),
        UpstreamSubscriptionError,
    > {
        if let Some((upstream_key, active_upstream)) =
            self.find_active_upstream_subscription(table, track_namespace, track_name)
        {
            return Ok((
                upstream_key,
                active_upstream,
                LargestObjectSource::LocalCache,
            ));
        }

        let (upstream_key, active_upstream) = self
            .create_upstream_subscription(
                session_id,
                track_namespace,
                track_name,
                table,
                forwarder,
                ingress_sender,
                upstream_publisher_resolver,
            )
            .await?;
        let largest_source =
            LargestObjectSource::SubscribeOk(active_upstream.content_exists.location());

        Ok((upstream_key, active_upstream, largest_source))
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe.find_active_upstream_subscription",
        skip_all,
        fields(
            track_namespace = %track_namespace,
            track_name = %track_name
        )
    )]
    fn find_active_upstream_subscription(
        &self,
        table: &dyn LocalPubSubDirectory,
        track_namespace: &str,
        track_name: &str,
    ) -> Option<(UpstreamSubscriptionKey, ActiveUpstreamSubscription)> {
        table
            .find_active_upstream_subscriptions(track_namespace, track_name)
            .into_iter()
            .min_by_key(|publisher| publisher.publisher_session_id)
            .and_then(|upstream_key| {
                let active_upstream = table.get_active_upstream_subscription(
                    upstream_key.publisher_session_id,
                    upstream_key.track_namespace.as_str(),
                    upstream_key.track_name.as_str(),
                )?;
                Some((upstream_key, active_upstream))
            })
    }

    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe.create_upstream_subscription",
        skip_all,
        fields(
            session_id = %session_id,
            track_namespace = %track_namespace,
            track_name = %track_name
        )
    )]
    async fn create_upstream_subscription(
        &self,
        session_id: SessionId,
        track_namespace: &str,
        track_name: &str,
        table: &dyn LocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
        upstream_publisher_resolver: &UpstreamPublisherResolver,
    ) -> Result<(UpstreamSubscriptionKey, ActiveUpstreamSubscription), UpstreamSubscriptionError>
    {
        let upstream_key = upstream_publisher_resolver
            .resolve(table, track_namespace, track_name)
            .await
            .map_err(|err| {
                tracing::warn!(
                    ?err,
                    track_namespace = %track_namespace,
                    track_name = %track_name,
                    "failed to resolve upstream publisher"
                );
                UpstreamSubscriptionError::PublisherNotFound
            })?
            .ok_or(UpstreamSubscriptionError::PublisherNotFound)?;

        let pub_session_id = upstream_key.publisher_session_id;
        let Ok(subscription) = forwarder
            .subscribe(
                pub_session_id,
                upstream_key.track_namespace.clone(),
                upstream_key.track_name.clone(),
            )
            .await
        else {
            tracing::warn!(
                pub_session_id = %pub_session_id,
                track_namespace = %upstream_key.track_namespace,
                track_name = %upstream_key.track_name,
                "failed to send upstream SUBSCRIBE"
            );
            return Err(UpstreamSubscriptionError::SubscribeFailed);
        };
        tracing::info!(
            pub_session_id = %pub_session_id,
            track_namespace = %upstream_key.track_namespace,
            track_name = %upstream_key.track_name,
            track_alias = subscription.track_alias(),
            expires = subscription.expires().unwrap_or(0),
            "upstream subscribe ok received"
        );

        let track_key = compose_session_track_key(pub_session_id, subscription.track_alias());
        let active_upstream = ActiveUpstreamSubscription {
            upstream_request_id: subscription.request_id(),
            track_key,
            expires: subscription.expires(),
            content_exists: subscription.content_exists(),
            downstream_subscriber_count: 0,
            origin: UpstreamSubscriptionOrigin::Subscribe,
        };

        if ingress_sender
            .send(IngressCommand::Start(Box::new(IngressStartRequest {
                subscriber_session_id: session_id,
                publisher_session_id: pub_session_id,
                track_namespace: upstream_key.track_namespace.clone(),
                track_name: upstream_key.track_name.clone(),
                subscription,
                parent_span: Span::current(),
            })))
            .await
            .is_err()
        {
            tracing::error!(
                pub_session_id = %pub_session_id,
                track_namespace = %upstream_key.track_namespace,
                track_name = %upstream_key.track_name,
                "failed to send ingress start request"
            );
            return Err(UpstreamSubscriptionError::IngressStartFailed);
        }
        table.register_upstream_subscription(upstream_key.clone(), active_upstream.clone());
        tracing::info!(
            pub_session_id = %pub_session_id,
            track_namespace = %upstream_key.track_namespace,
            track_name = %upstream_key.track_name,
            "upstream subscription registered"
        );

        Ok((upstream_key, active_upstream))
    }

    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe.accept_downstream_subscription",
        skip_all,
        fields(
            session_id = %session_id
        )
    )]
    async fn accept_downstream_subscription(
        &self,
        session_id: SessionId,
        upstream_key: UpstreamSubscriptionKey,
        active_upstream: ActiveUpstreamSubscription,
        largest_source: LargestObjectSource,
        table: &dyn LocalPubSubDirectory,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        cache_store: &Arc<TrackCacheStore>,
        handler: &dyn SubscribeHandler,
    ) {
        let subscriber_track_alias = handler.allocate_track_alias();

        // Determined here so the Largest Location advertised in SUBSCRIBE_OK
        // and the egress delivery start agree.
        let largest_location = match &largest_source {
            LargestObjectSource::SubscribeOk(location) => *location,
            LargestObjectSource::LocalCache => match cache_store.get(active_upstream.track_key) {
                Some(cache) => cache.largest_location().await,
                None => None,
            },
        };
        let content_exists = match &largest_location {
            Some(loc) => ContentExists::True {
                location: Location {
                    group_id: loc.group_id,
                    object_id: loc.object_id,
                },
            },
            None => active_upstream.content_exists.clone(),
        };

        if !table.register_downstream_subscription(
            session_id,
            handler.subscribe_id(),
            upstream_key.clone(),
            largest_location,
        ) {
            tracing::error!(
                subscribe_id = handler.subscribe_id(),
                track_namespace = %upstream_key.track_namespace,
                track_name = %upstream_key.track_name,
                "failed to register downstream subscription"
            );
            return;
        }

        let (ready_sender, ready_receiver) = tokio::sync::oneshot::channel();
        if egress_sender
            .send(EgressCommand::StartReader(Box::new(EgressStartRequest {
                subscriber_session_id: session_id,
                downstream_subscribe_id: handler.subscribe_id(),
                track_key: active_upstream.track_key,
                track_namespace: upstream_key.track_namespace.clone(),
                track_name: upstream_key.track_name.clone(),
                downstream_subscription: handler.to_downstream_subscription(subscriber_track_alias),
                parent_span: Span::current(),
                ready_sender,
                largest_location,
            })))
            .await
            .is_err()
        {
            tracing::error!(
                subscribe_id = handler.subscribe_id(),
                track_namespace = %upstream_key.track_namespace,
                track_name = %upstream_key.track_name,
                "failed to send EgressStartRequest"
            );
            return;
        }
        match ready_receiver.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::error!(
                    ?error,
                    subscribe_id = handler.subscribe_id(),
                    "failed to start egress runner"
                );
                return;
            }
            Err(error) => {
                tracing::error!(
                    ?error,
                    subscribe_id = handler.subscribe_id(),
                    "egress runner readiness dropped"
                );
                return;
            }
        }

        if handler
            .ok_with_track_alias(
                subscriber_track_alias,
                active_upstream.expires.unwrap_or(0),
                content_exists,
            )
            .await
            .is_err()
        {
            tracing::error!(
                subscribe_id = handler.subscribe_id(),
                subscriber_track_alias = subscriber_track_alias,
                "failed to send SUBSCRIBE_OK"
            );
            // TODO: send_unsubscribe
            // TODO: close session
            return;
        }
        tracing::info!(
            session_id = %session_id,
            track_namespace = %upstream_key.track_namespace,
            track_name = %upstream_key.track_name,
            subscriber_track_alias = subscriber_track_alias,
            "downstream subscribe ok sent"
        );
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe.response_error",
        skip_all
    )]
    async fn response_error(
        &self,
        handler: &dyn SubscribeHandler,
        code: u64,
        reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        let track_namespace = handler.track_namespace();
        let track_name = handler.track_name();
        tracing::warn!(
            subscribe_id = handler.subscribe_id(),
            track_namespace = %track_namespace,
            track_name = %track_name,
            error_code = code,
            reason_phrase = %reason_phrase,
            "Sending `SUBSCRIBE_ERROR`"
        );
        handler.error(code, reason_phrase).await
    }
}
