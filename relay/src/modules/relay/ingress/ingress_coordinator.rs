use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{Instrument, Span};

use crate::modules::{
    core::{data_receiver::receiver::DataReceiver, subscription::Subscription},
    relay::{
        cache::store::TrackCacheStore,
        ingress::{
            datagram_reader::{DatagramReader, DatagramReceiveStart},
            stream_ingress_task::{StreamIngressTask, StreamReceiveStart},
        },
        notifications::track_notifier::ObjectNotifyProducerMap,
    },
    session_repository::SessionRepository,
    types::{SessionId, compose_session_track_key},
};

pub(crate) struct IngressStartRequest {
    pub(crate) publisher_session_id: SessionId,
    pub(crate) subscription: Subscription,
    pub(crate) parent_span: Span,
}

pub(crate) struct IngressCoordinator {
    command_sender: mpsc::Sender<IngressStartRequest>,
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
        let (stream_tx, stream_rx) = mpsc::channel::<StreamReceiveStart>(64);
        let (datagram_tx, datagram_rx) = mpsc::channel::<DatagramReceiveStart>(64);
        let stream_task = StreamIngressTask::new(
            stream_rx,
            cache_store.clone(),
            object_notify_producer_map.clone(),
        );
        let datagram_reader =
            DatagramReader::run(datagram_rx, cache_store, object_notify_producer_map);

        let (command_sender, mut command_receiver) = mpsc::channel::<IngressStartRequest>(512);
        let session_repo_for_runner = session_repo;

        let command_runner = tokio::spawn(async move {
            let mut join_set = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    Some(command) = command_receiver.recv() => {
                        let track_key = compose_session_track_key(
                            command.publisher_session_id,
                            command.subscription.track_alias(),
                        );
                        let Some(subscriber) = session_repo_for_runner.lock().await.subscriber(command.publisher_session_id) else {
                            tracing::debug!(track_key, "publisher session not found for subscription");
                            continue;
                        };
                        let stream_tx = stream_tx.clone();
                        let datagram_tx = datagram_tx.clone();
                        let create_receiver_span = tracing::info_span!(
                            parent: &command.parent_span,
                            "relay.ingress.create_data_receiver",
                            publisher_session_id = command.publisher_session_id,
                            track_key = track_key,
                            track_alias = command.subscription.track_alias(),
                        );
                        join_set.spawn(async move {
                            let subscription = command.subscription;
                            let mut subscriber = subscriber;
                            let Ok(receiver) = subscriber.create_data_receiver(&subscription).await else {
                                tracing::debug!(track_key, "failed to create data receiver for subscription");
                                return;
                            };
                            match receiver {
                                DataReceiver::Stream(factory) => {
                                    let _ = stream_tx
                                        .send(StreamReceiveStart { track_key, factory })
                                        .await;
                                }
                                DataReceiver::Datagram(datagram_receiver) => {
                                    let _ = datagram_tx
                                        .send(DatagramReceiveStart { track_key, receiver: datagram_receiver })
                                        .await;
                                }
                            }
                        }.instrument(create_receiver_span));
                    }
                    Some(join_result) = join_set.join_next() => {
                        if let Err(error) = join_result {
                            tracing::debug!(?error, "a task in ingress coordinator failed");
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

    pub(crate) fn sender(&self) -> mpsc::Sender<IngressStartRequest> {
        self.command_sender.clone()
    }
}

impl Drop for IngressCoordinator {
    fn drop(&mut self) {
        self.command_runner.abort();
    }
}
