use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use crate::modules::{
    core::{data_receiver::receiver::DataReceiver, subscription::Subscription},
    event_resolver::stream_runner::StreamTaskRunner,
    relay::{
        cache::store::TrackCacheStore,
        ingest::{receiver_registry::ReceiverRegistry, writer::CacheWriter},
        types::IngressTransportNotification,
    },
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

pub(crate) enum IngestCommand {
    StartBySubscribe {
        request: IngestStartRequest,
        response: oneshot::Sender<anyhow::Result<IngestStartResult>>,
    },
    RegisterDataReceiver {
        track_key: TrackKey,
        receiver: DataReceiver,
    },
}

pub(crate) struct IngestCoordinator {
    command_sender: mpsc::Sender<IngestCommand>,
    command_runner: tokio::task::JoinHandle<()>,
    receiver_registry: Arc<ReceiverRegistry>,
    cache_writer: CacheWriter,
    stream_runner: Arc<StreamTaskRunner>,
}

impl IngestCoordinator {
    pub(crate) fn new(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        cache_store: Arc<TrackCacheStore>,
        transport_notifier: mpsc::Sender<IngressTransportNotification>,
    ) -> Self {
        let cache_writer = CacheWriter::start(cache_store, transport_notifier, 1024);
        let receiver_registry = Arc::new(ReceiverRegistry::new(cache_writer.sender()));
        let stream_runner = Arc::new(StreamTaskRunner::new());

        let (command_sender, mut command_receiver) = mpsc::channel::<IngestCommand>(512);
        let command_sender_for_runner = command_sender.clone();
        let receiver_registry_for_runner = receiver_registry.clone();
        let stream_runner_for_runner = stream_runner.clone();
        let session_repo_for_runner = session_repo.clone();

        let command_runner = tokio::spawn(async move {
            while let Some(command) = command_receiver.recv().await {
                match command {
                    IngestCommand::StartBySubscribe { request, response } => {
                        let result = Self::handle_start_by_subscribe(
                            session_repo_for_runner.clone(),
                            command_sender_for_runner.clone(),
                            stream_runner_for_runner.clone(),
                            request,
                        )
                        .await;
                        if response.send(result).is_err() {
                            tracing::warn!("ingest start response receiver dropped");
                        }
                    }
                    IngestCommand::RegisterDataReceiver {
                        track_key,
                        receiver,
                    } => {
                        receiver_registry_for_runner.register_data_receiver(track_key, receiver);
                    }
                }
            }
        });

        Self {
            command_sender,
            command_runner,
            receiver_registry,
            cache_writer,
            stream_runner,
        }
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<IngestCommand> {
        self.command_sender.clone()
    }

    pub(crate) fn is_running(&self) -> bool {
        !self.command_runner.is_finished() && self.stream_runner.is_running()
    }

    pub(crate) fn track_count(&self) -> usize {
        self.receiver_registry.track_count()
    }

    async fn handle_start_by_subscribe(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        command_sender: mpsc::Sender<IngestCommand>,
        stream_runner: Arc<StreamTaskRunner>,
        request: IngestStartRequest,
    ) -> anyhow::Result<IngestStartResult> {
        let mut subscriber = session_repo
            .lock()
            .await
            .subscriber(request.publisher_session_id)
            .ok_or_else(|| anyhow::anyhow!("subscriber session not found"))?;

        let mut subscription = request.subscription;
        let receiver = subscriber.create_data_receiver(subscription.as_mut()).await?;

        let upstream_track_alias = receiver.get_track_alias();
        let track_key = compose_session_track_key(request.publisher_session_id, upstream_track_alias);
        let is_stream = matches!(&receiver, DataReceiver::Stream(_));

        command_sender
            .send(IngestCommand::RegisterDataReceiver {
                track_key,
                receiver,
            })
            .await
            .map_err(|error| anyhow::anyhow!("failed to register initial receiver: {error}"))?;

        tracing::debug!(
            track_key,
            upstream_track_alias,
            is_stream,
            "relay ingest pipeline started"
        );

        if is_stream {
            let command_sender_for_streams = command_sender.clone();
            let stream_accept_task = async move {
                let mut subscription = subscription;
                loop {
                    let receiver = match subscriber.create_data_receiver(subscription.as_mut()).await {
                        Ok(receiver) => receiver,
                        Err(error) => {
                            tracing::debug!(?error, track_key, "stream accept loop finished");
                            return;
                        }
                    };

                    match receiver {
                        DataReceiver::Stream(_) => {
                            if command_sender_for_streams
                                .send(IngestCommand::RegisterDataReceiver {
                                    track_key,
                                    receiver,
                                })
                                .await
                                .is_err()
                            {
                                tracing::error!(
                                    track_key,
                                    "failed to dispatch stream receiver to ingest"
                                );
                                return;
                            }
                        }
                        DataReceiver::Datagram(_) => {
                            tracing::error!(
                                track_key,
                                "expected stream receiver but got datagram receiver"
                            );
                            return;
                        }
                    }
                }
            };
            stream_runner.add_task(Box::pin(stream_accept_task)).await;
        }

        Ok(IngestStartResult { track_key })
    }
}

impl Drop for IngestCoordinator {
    fn drop(&mut self) {
        drop(self.cache_writer.sender());
        self.command_runner.abort();
    }
}
