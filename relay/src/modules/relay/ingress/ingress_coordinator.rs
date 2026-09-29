use std::{collections::HashMap, sync::Arc};

use opentelemetry::trace::TraceContextExt as _;
use tokio::sync::{mpsc, watch};
use tracing::{Instrument, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::modules::{
    core::{data_receiver::receiver::DataReceiver, subscription::UpstreamSubscription},
    relay::{
        cache::store::TrackCacheStore,
        ingress::track_ingest_task::{IngestCommand, IngestSource, IngestStart, TrackIngestTask},
    },
    session_event::SessionEvent,
    session_repository::SessionRepository,
    types::{SessionId, TrackKey},
};

pub(crate) struct IngressStartRequest {
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) publisher_session_id: SessionId,
    pub(crate) track_key: TrackKey,
    pub(crate) subscription: UpstreamSubscription,
    pub(crate) parent_span: Span,
}

pub(crate) enum IngressCommand {
    Start(Box<IngressStartRequest>),
    StopTrack {
        track_key: TrackKey,
        publisher_session_id: SessionId,
    },
}

pub(crate) struct IngressCoordinator {
    command_sender: mpsc::Sender<IngressCommand>,
    command_runner: tokio::task::JoinHandle<()>,
    _track_ingest: TrackIngestTask,
}

impl IngressCoordinator {
    pub(crate) fn new(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        cache_store: Arc<TrackCacheStore>,
        session_event_sender: mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        let (ingest_tx, ingest_rx) = mpsc::channel::<IngestCommand>(64);
        let track_ingest = TrackIngestTask::run(ingest_rx, cache_store, session_event_sender);

        let (command_sender, mut command_receiver) = mpsc::channel::<IngressCommand>(512);

        let command_runner = tokio::spawn(async move {
            let mut join_set = tokio::task::JoinSet::new();
            let mut create_stop_senders = HashMap::<TrackKey, watch::Sender<bool>>::new();
            loop {
                tokio::select! {
                    Some(command) = command_receiver.recv() => {
                        match command {
                        IngressCommand::Start(command) => {
                        let track_key = command.track_key.clone();
                        tracing::info!(
                            track_key = %track_key,
                            subscriber_session_id = %command.subscriber_session_id,
                            publisher_session_id = %command.publisher_session_id,
                            track_alias = command.subscription.track_alias(),
                            track_namespace = %track_key.track_namespace,
                            track_name = %track_key.track_name,
                            "ingress start command received"
                        );
                        let (subscriber, publisher_session_span) = {
                            let session_repo = session_repo.lock().await;
                            let Some(subscriber) = session_repo.subscriber(command.publisher_session_id) else {
                                tracing::info!(%track_key, "publisher session not found for subscription");
                                continue;
                            };
                            let publisher_session_span = session_repo
                                .session_span(command.publisher_session_id)
                                .unwrap_or_else(|| command.parent_span.clone());
                            (subscriber, publisher_session_span)
                        };
                        tracing::info!(%track_key, "upstream subscriber found; spawning data receiver task");
                        let ingest_tx = ingest_tx.clone();
                        if let Some(stop_sender) = create_stop_senders.remove(&track_key) {
                            let _ = stop_sender.send(true);
                        }
                        let (create_stop_sender, mut create_stop_receiver) = watch::channel(false);
                        create_stop_senders.insert(track_key.clone(), create_stop_sender);
                        let create_receiver_span = tracing::info_span!(
                            parent: &command.parent_span,
                            "relay.upstream.ingress",
                            subscriber_session_id = command.subscriber_session_id,
                            publisher_session_id = command.publisher_session_id,
                            track_key = %track_key,
                            track_alias = command.subscription.track_alias(),
                            track_namespace = %track_key.track_namespace,
                            track_name = %track_key.track_name,
                        );
                        create_receiver_span.add_link(
                            publisher_session_span
                                .context()
                                .span()
                                .span_context()
                                .clone(),
                        );
                        join_set.spawn(async move {
                            let subscription = command.subscription;
                            let mut subscriber = subscriber;
                            tracing::info!(%track_key, "creating upstream data receiver");
                            let receiver_result = tokio::select! {
                                _ = create_stop_receiver.changed() => {
                                    tracing::info!(%track_key, "upstream ingress stopped");
                                    return track_key;
                                }
                                receiver = subscriber.create_data_receiver(&subscription) => receiver,
                            };
                            let Ok(receiver) = receiver_result else {
                                tracing::info!(%track_key, "failed to start upstream ingress");
                                return track_key;
                            };
                            tracing::info!(%track_key, "upstream data receiver created");
                            let source = match receiver {
                                DataReceiver::Stream(factory) => {
                                    let dataplane_track_span = tracing::info_span!(
                                        parent: &publisher_session_span,
                                        "relay.dataplane.ingress.track",
                                        subscriber_session_id = command.subscriber_session_id,
                                        publisher_session_id = command.publisher_session_id,
                                        track_key = %track_key,
                                        track_alias = subscription.track_alias(),
                                        track_namespace = %track_key.track_namespace,
                                        track_name = %track_key.track_name,
                                    );
                                    dataplane_track_span.add_link(
                                        command
                                            .parent_span
                                            .context()
                                            .span()
                                            .span_context()
                                            .clone(),
                                    );
                                    IngestSource::Stream { factory, track_span: dataplane_track_span }
                                }
                                DataReceiver::Datagram(datagram_receiver) => IngestSource::Datagram(datagram_receiver),
                            };
                            let kind = source.kind().label();
                            if ingest_tx
                                .send(IngestCommand::Start(IngestStart {
                                    track_key: track_key.clone(),
                                    publisher_session_id: command.publisher_session_id,
                                    source,
                                }))
                                .await
                                .is_ok()
                            {
                                tracing::info!(%track_key, "{kind} ingress start command sent");
                            } else {
                                tracing::info!(%track_key, "failed to send {kind} ingress start command");
                            }
                            track_key
                        }.instrument(create_receiver_span));
                        }
                        IngressCommand::StopTrack { track_key, publisher_session_id } => {
                            if let Some(stop_sender) = create_stop_senders.remove(&track_key) {
                                let _ = stop_sender.send(true);
                                tracing::info!(%track_key, "upstream ingress stop requested");
                            }
                            if ingest_tx
                                .send(IngestCommand::Stop {
                                    track_key: track_key.clone(),
                                    publisher_session_id,
                                })
                                .await
                                .is_err()
                            {
                                tracing::debug!(%track_key, "failed to send ingress stop request");
                            }
                        }
                        }
                    }
                    Some(join_result) = join_set.join_next() => {
                        match join_result {
                            Ok(track_key) => {
                                create_stop_senders.remove(&track_key);
                                tracing::debug!(%track_key, "upstream ingress task ended");
                            }
                            Err(error) => {
                                tracing::debug!(?error, "a task in ingress coordinator failed");
                            }
                        }
                    }
                }
            }
        });

        Self {
            command_sender,
            command_runner,
            _track_ingest: track_ingest,
        }
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<IngressCommand> {
        self.command_sender.clone()
    }
}

impl Drop for IngressCoordinator {
    fn drop(&mut self) {
        self.command_runner.abort();
    }
}
