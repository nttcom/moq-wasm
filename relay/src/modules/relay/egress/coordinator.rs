use std::sync::Arc;

use tokio::sync::mpsc;

use crate::modules::{
    core::published_resource::PublishedResource,
    event_resolver::stream_runner::StreamTaskRunner,
    relay::{
        cache::store::TrackCacheStore,
        egress::{reader::SubscriberAcceptReader, stream_allocator::StreamAllocator},
        types::{IngressTransportNotification, RelayTransport},
    },
    session_repository::SessionRepository,
    types::{SessionId, TrackKey},
};

pub(crate) struct EgressStartRequest {
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) track_key: TrackKey,
    pub(crate) published_resources: PublishedResource,
}

pub(crate) enum EgressCommand {
    StartReader(EgressStartRequest),
}

pub(crate) struct EgressCoordinator {
    command_sender: mpsc::Sender<EgressCommand>,
    command_runner: tokio::task::JoinHandle<()>,
    stream_runner: Arc<StreamTaskRunner>,
}

impl EgressCoordinator {
    pub(crate) fn new(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        cache_store: Arc<TrackCacheStore>,
        mut transport_receiver: mpsc::Receiver<IngressTransportNotification>,
    ) -> Self {
        let mut pending_requests = std::collections::HashMap::<TrackKey, Vec<EgressStartRequest>>::new();
        let mut known_transport = std::collections::HashMap::<TrackKey, RelayTransport>::new();
        let stream_runner = Arc::new(StreamTaskRunner::new());
        let (command_sender, mut command_receiver) = mpsc::channel::<EgressCommand>(512);

        let stream_runner_for_runner = stream_runner.clone();
        let cache_store_for_runner = cache_store.clone();
        let session_repo_for_runner = session_repo.clone();
        let command_runner = tokio::spawn(async move {
            loop {
                tokio::select! {
                    maybe_command = command_receiver.recv() => {
                        let Some(command) = maybe_command else {
                            break;
                        };
                        match command {
                            EgressCommand::StartReader(request) => {
                                if let Some(transport) = known_transport.get(&request.track_key).cloned() {
                                    Self::spawn_reader(
                                        session_repo_for_runner.clone(),
                                        cache_store_for_runner.clone(),
                                        stream_runner_for_runner.clone(),
                                        request,
                                        transport,
                                    )
                                    .await;
                                } else {
                                    pending_requests
                                        .entry(request.track_key)
                                        .or_default()
                                        .push(request);
                                }
                            }
                        }
                    }
                    maybe_transport = transport_receiver.recv() => {
                        let Some(notification) = maybe_transport else {
                            break;
                        };
                        known_transport.insert(notification.track_key, notification.transport.clone());
                        if let Some(requests) = pending_requests.remove(&notification.track_key) {
                            for request in requests {
                                Self::spawn_reader(
                                    session_repo_for_runner.clone(),
                                    cache_store_for_runner.clone(),
                                    stream_runner_for_runner.clone(),
                                    request,
                                    notification.transport.clone(),
                                )
                                .await;
                            }
                        }
                    }
                }
            }
        });

        Self {
            command_sender,
            command_runner,
            stream_runner,
        }
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<EgressCommand> {
        self.command_sender.clone()
    }

    pub(crate) fn is_running(&self) -> bool {
        !self.command_runner.is_finished() && self.stream_runner.is_running()
    }

    async fn spawn_reader(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        cache_store: Arc<TrackCacheStore>,
        stream_runner: Arc<StreamTaskRunner>,
        request: EgressStartRequest,
        transport: RelayTransport,
    ) {
        let publisher = session_repo
            .lock()
            .await
            .publisher(request.subscriber_session_id);
        let Some(publisher) = publisher else {
            tracing::error!("publisher session not found for egress start");
            return;
        };

        let egress_task = async move {
            let cache = cache_store.get_or_create(request.track_key);
            let subscriber_track_alias = request.published_resources.track_alias();
            let group_order = request.published_resources.group_order();
            let filter_type = request.published_resources.filter_type();

            let mut reader = SubscriberAcceptReader::new(
                request.track_key,
                cache,
                &filter_type,
                group_order,
                transport,
            )
            .await;
            let allocator = StreamAllocator::new(publisher.as_ref(), &request.published_resources);
            if let Err(error) = reader
                .run_with_allocator(&allocator, subscriber_track_alias)
                .await
            {
                tracing::error!(
                    ?error,
                    track_key = request.track_key,
                    "egress relay loop finished with error"
                );
            }
        };

        stream_runner.add_task(Box::pin(egress_task)).await;
    }
}

impl Drop for EgressCoordinator {
    fn drop(&mut self) {
        self.command_runner.abort();
    }
}
