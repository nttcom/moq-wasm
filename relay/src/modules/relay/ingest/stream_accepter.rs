use std::sync::Arc;

use crate::modules::{
    core::{data_receiver::receiver::DataReceiver, subscription::Subscription},
    event_resolver::stream_runner::StreamTaskRunner,
    relay::{cache::store::TrackCacheStore, ingest::writer::CacheWriter},
    session_repository::SessionRepository,
    types::{SessionId, TrackKey, compose_session_track_key},
};

pub(crate) struct IngestStartRequest {
    pub(crate) publisher_session_id: SessionId,
    pub(crate) subscription: Box<dyn Subscription>,
}

pub(crate) struct IngestStartResult {
    pub(crate) track_key: TrackKey,
}

pub(crate) struct IngestCoordinator {
    command_runner: tokio::task::JoinHandle<()>,
    cache_writer: CacheWriter,
    stream_runner: Arc<StreamTaskRunner>,
}

impl IngestCoordinator {
    pub(crate) fn new(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        cache_store: Arc<TrackCacheStore>,
    ) -> Self {
        let cache_writer = CacheWriter::start(cache_store, transport_notifier, 1024);
        let stream_runner = Arc::new(StreamTaskRunner::new());

        let (command_sender, mut command_receiver) =
            tokio::sync::mpsc::channel::<IngestStartRequest>(512);
        let session_repo_for_runner = session_repo.clone();

        let command_runner = tokio::spawn(async move {
            let mut join_set = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    Some(command) = command_receiver.recv() => {
                        let track_key = compose_session_track_key(command.publisher_session_id, command.subscription.track_alias());
                        let Some(subscriber) = session_repo_for_runner.lock().await.subscriber(command.publisher_session_id) else {
                            tracing::debug!(
                                track_key,
                                "publisher session not found for subscription"
                            );
                            continue;
                        };
                        join_set.spawn(async move {
                            let mut subscription = command.subscription;
                            let mut subscriber = subscriber;
                            loop {
                                let Ok(receiver) = subscriber.create_data_receiver(subscription.as_mut()).await else {
                                    tracing::debug!(
                                            track_key,
                                            "failed to create data receiver for subscription"
                                        );
                                        return;
                                };
                                match receiver {
                                    DataReceiver::Stream(stream_receiver) => {
                                        // send stream
                                        // loop
                                    }
                                    DataReceiver::Datagram(datagram_receiver) => {
                                        // send datagram
                                        // break
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
            command_runner,
            cache_writer,
            stream_runner,
        }
    }
}

impl Drop for IngestCoordinator {
    fn drop(&mut self) {
        drop(self.cache_writer.sender());
        self.command_runner.abort();
    }
}
