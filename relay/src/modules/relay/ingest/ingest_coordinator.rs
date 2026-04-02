use std::sync::Arc;

use tokio::sync::mpsc;

use crate::modules::{
    core::{data_receiver::receiver::DataReceiver, subscription::Subscription},
    relay::{
        cache::store::TrackCacheStore,
        notifications::{
            delivery_type_map::DeliveryTypeMap,
            sender_map::SenderMap,
        },
        ingest::{
            datagram_ingest_task::{DatagramIngestTask, DatagramReceiveStart},
            stream_ingest_task::{StreamReceiveStart, StreamIngestTask},
        },
    },
    session_repository::SessionRepository,
    types::{SessionId, compose_session_track_key},
};

pub(crate) struct IngestStartRequest {
    pub(crate) publisher_session_id: SessionId,
    pub(crate) subscription: Box<dyn Subscription>,
}

pub(crate) struct IngestCoordinator {
    command_sender: mpsc::Sender<IngestStartRequest>,
    command_runner: tokio::task::JoinHandle<()>,
    _stream_task: StreamIngestTask,
    _datagram_task: DatagramIngestTask,
}

impl IngestCoordinator {
    pub(crate) fn new(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        cache_store: Arc<TrackCacheStore>,
        sender_map: Arc<SenderMap>,
        delivery_type_map: Arc<DeliveryTypeMap>,
    ) -> Self {
        let (stream_tx, stream_rx) = mpsc::channel::<StreamReceiveStart>(64);
        let (datagram_tx, datagram_rx) = mpsc::channel::<DatagramReceiveStart>(64);
        let stream_task = StreamIngestTask::new(stream_rx, cache_store.clone(), sender_map.clone(), delivery_type_map.clone());
        let datagram_task = DatagramIngestTask::new(datagram_rx, cache_store, sender_map, delivery_type_map);

        let (command_sender, mut command_receiver) = mpsc::channel::<IngestStartRequest>(512);
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
                        join_set.spawn(async move {
                            let mut subscription = command.subscription;
                            let mut subscriber = subscriber;
                            loop {
                                let Ok(receiver) = subscriber.create_data_receiver(subscription.as_mut()).await else {
                                    tracing::debug!(track_key, "failed to create data receiver for subscription");
                                    return;
                                };
                                match receiver {
                                    DataReceiver::Stream(stream_receiver) => {
                                        if stream_tx
                                            .send(StreamReceiveStart { track_key, receiver: stream_receiver })
                                            .await
                                            .is_err()
                                        {
                                            return;
                                        }
                                        // loop: 次のStreamを待つ
                                    }
                                    DataReceiver::Datagram(datagram_receiver) => {
                                        let _ = datagram_tx
                                            .send(DatagramReceiveStart { track_key, receiver: datagram_receiver })
                                            .await;
                                        break;
                                    }
                                }
                            }
                        });
                    }
                    Some(join_result) = join_set.join_next() => {
                        if let Err(error) = join_result {
                            tracing::debug!(?error, "a task in ingest coordinator failed");
                        }
                    }
                }
            }
        });

        Self {
            command_sender,
            command_runner,
            _stream_task: stream_task,
            _datagram_task: datagram_task,
        }
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<IngestStartRequest> {
        self.command_sender.clone()
    }

    pub(crate) fn is_running(&self) -> bool {
        !self.command_runner.is_finished()
    }

    pub(crate) fn track_count(&self) -> usize {
        0
    }
}

impl Drop for IngestCoordinator {
    fn drop(&mut self) {
        self.command_runner.abort();
    }
}
