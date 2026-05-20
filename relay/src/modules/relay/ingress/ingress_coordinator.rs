use std::{collections::HashMap, sync::Arc};

use opentelemetry::trace::TraceContextExt as _;
use tokio::sync::{mpsc, watch};
use tracing::{Instrument, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::modules::{
    core::{data_receiver::receiver::DataReceiver, subscription::Subscription},
    relay::{
        cache::store::TrackCacheStore,
        ingress::{
            datagram_reader::{DatagramReader, DatagramReceiveCommand, DatagramReceiveStart},
            stream_ingress_task::{StreamIngressCommand, StreamIngressTask, StreamReceiveStart},
        },
        notifications::track_notifier::ObjectNotifyProducerMap,
    },
    session_repository::SessionRepository,
    types::{SessionId, TrackKey, compose_session_track_key},
};

pub(crate) struct IngressStartRequest {
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) publisher_session_id: SessionId,
    pub(crate) track_namespace: String,
    pub(crate) track_name: String,
    pub(crate) subscription: Subscription,
    pub(crate) parent_span: Span,
}

pub(crate) enum IngressCommand {
    Start(IngressStartRequest),
    StopTrack { track_key: TrackKey },
}

pub(crate) struct IngressCoordinator {
    command_sender: mpsc::Sender<IngressCommand>,
    command_runner: tokio::task::JoinHandle<()>,
    _stream_task: StreamIngressTask,
    _datagram_reader: DatagramReader,
}

impl IngressCoordinator {
    pub(crate) fn new(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        cache_store: Arc<TrackCacheStore>,
        object_notify_producer_map: Arc<ObjectNotifyProducerMap>,
    ) -> Self {
        let (stream_tx, stream_rx) = mpsc::channel::<StreamIngressCommand>(64);
        let (datagram_tx, datagram_rx) = mpsc::channel::<DatagramReceiveCommand>(64);
        let stream_task = StreamIngressTask::new(
            stream_rx,
            cache_store.clone(),
            object_notify_producer_map.clone(),
        );
        let datagram_reader =
            DatagramReader::run(datagram_rx, cache_store, object_notify_producer_map);

        let (command_sender, mut command_receiver) = mpsc::channel::<IngressCommand>(512);
        let session_repo_for_runner = session_repo;

        let command_runner = tokio::spawn(async move {
            let mut join_set = tokio::task::JoinSet::new();
            let mut create_stop_senders = HashMap::<TrackKey, watch::Sender<bool>>::new();
            loop {
                tokio::select! {
                    Some(command) = command_receiver.recv() => {
                        match command {
                        IngressCommand::Start(command) => {
                        let track_key = compose_session_track_key(
                            command.publisher_session_id,
                            command.subscription.track_alias(),
                        );
                        let (subscriber, publisher_session_span) = {
                            let session_repo = session_repo_for_runner.lock().await;
                            let Some(subscriber) = session_repo.subscriber(command.publisher_session_id) else {
                                tracing::debug!(track_key, "publisher session not found for subscription");
                                continue;
                            };
                            let publisher_session_span = session_repo
                                .session_span(command.publisher_session_id)
                                .unwrap_or_else(|| command.parent_span.clone());
                            (subscriber, publisher_session_span)
                        };
                        let stream_tx = stream_tx.clone();
                        let datagram_tx = datagram_tx.clone();
                        if let Some(stop_sender) = create_stop_senders.remove(&track_key) {
                            let _ = stop_sender.send(true);
                        }
                        let (create_stop_sender, mut create_stop_receiver) = watch::channel(false);
                        create_stop_senders.insert(track_key, create_stop_sender);
                        let create_receiver_span = tracing::info_span!(
                            parent: &command.parent_span,
                            "relay.upstream.ingress",
                            subscriber_session_id = command.subscriber_session_id,
                            publisher_session_id = command.publisher_session_id,
                            track_key = track_key,
                            track_alias = command.subscription.track_alias(),
                            track_namespace = %command.track_namespace,
                            track_name = %command.track_name,
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
                            let receiver_result = tokio::select! {
                                _ = create_stop_receiver.changed() => {
                                    tracing::info!(track_key, "upstream ingress stopped");
                                    return track_key;
                                }
                                receiver = subscriber.create_data_receiver(&subscription) => receiver,
                            };
                            let Ok(receiver) = receiver_result else {
                                tracing::debug!(track_key, "failed to start upstream ingress");
                                return track_key;
                            };
                            match receiver {
                                DataReceiver::Stream(factory) => {
                                    let dataplane_track_span = tracing::info_span!(
                                        parent: &publisher_session_span,
                                        "relay.dataplane.ingress.track",
                                        subscriber_session_id = command.subscriber_session_id,
                                        publisher_session_id = command.publisher_session_id,
                                        track_key = track_key,
                                        track_alias = subscription.track_alias(),
                                        track_namespace = %command.track_namespace,
                                        track_name = %command.track_name,
                                    );
                                    dataplane_track_span.add_link(
                                        command
                                            .parent_span
                                            .context()
                                            .span()
                                            .span_context()
                                            .clone(),
                                    );
                                    let _ = stream_tx
                                        .send(StreamIngressCommand::Start(StreamReceiveStart {
                                            track_key,
                                            factory,
                                            track_span: dataplane_track_span,
                                        }))
                                        .await;
                                }
                                DataReceiver::Datagram(datagram_receiver) => {
                                    let _ = datagram_tx
                                        .send(DatagramReceiveCommand::Start(DatagramReceiveStart {
                                            track_key,
                                            receiver: datagram_receiver,
                                        }))
                                        .await;
                                }
                            }
                            track_key
                        }.instrument(create_receiver_span));
                        }
                        IngressCommand::StopTrack { track_key } => {
                            if let Some(stop_sender) = create_stop_senders.remove(&track_key) {
                                let _ = stop_sender.send(true);
                                tracing::info!(track_key, "upstream ingress stop requested");
                            }
                            let stream_result = stream_tx
                                .send(StreamIngressCommand::Stop { track_key })
                                .await;
                            let datagram_result = datagram_tx
                                .send(DatagramReceiveCommand::Stop { track_key })
                                .await;
                            if stream_result.is_err() || datagram_result.is_err() {
                                tracing::debug!(track_key, "failed to send ingress stop request");
                            }
                        }
                        }
                    }
                    Some(join_result) = join_set.join_next() => {
                        match join_result {
                            Ok(track_key) => {
                                create_stop_senders.remove(&track_key);
                                tracing::debug!(track_key, "upstream ingress task ended");
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
            _stream_task: stream_task,
            _datagram_reader: datagram_reader,
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
