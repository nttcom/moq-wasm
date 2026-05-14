use std::{collections::HashMap, sync::Arc};

use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    core::published_resource::PublishedResource,
    relay::{
        cache::store::TrackCacheStore, egress::runner::EgressRunner,
        notifications::track_notifier::ObjectNotifyProducerMap,
    },
    session_repository::SessionRepository,
    types::{SessionId, TrackKey},
};

pub(crate) struct EgressStartRequest {
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) downstream_subscribe_id: u64,
    pub(crate) track_key: TrackKey,
    pub(crate) published_resources: PublishedResource,
}

pub(crate) enum EgressCommand {
    StartReader(EgressStartRequest),
    StopReader {
        subscriber_session_id: SessionId,
        downstream_subscribe_id: u64,
    },
}

pub(crate) struct EgressCoordinator {
    command_sender: mpsc::Sender<EgressCommand>,
    command_runner: tokio::task::JoinHandle<()>,
}

impl EgressCoordinator {
    pub(crate) fn new(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        cache_store: Arc<TrackCacheStore>,
        object_notify_producer_map: Arc<ObjectNotifyProducerMap>,
    ) -> Self {
        let (command_sender, mut command_receiver) = mpsc::channel::<EgressCommand>(512);

        let command_runner = tokio::spawn(async move {
            let mut runners = HashMap::<(SessionId, u64), JoinHandle<()>>::new();
            loop {
                let Some(command) = command_receiver.recv().await else {
                    break;
                };
                match command {
                    EgressCommand::StartReader(request) => {
                        let runner_key = (
                            request.subscriber_session_id,
                            request.downstream_subscribe_id,
                        );
                        if let Some(existing) = runners.remove(&runner_key) {
                            existing.abort();
                        }
                        if let Some(handle) = Self::spawn_runner(
                            session_repo.clone(),
                            cache_store.clone(),
                            object_notify_producer_map.clone(),
                            request,
                        )
                        .await
                        {
                            runners.insert(runner_key, handle);
                        }
                    }
                    EgressCommand::StopReader {
                        subscriber_session_id,
                        downstream_subscribe_id,
                    } => {
                        if let Some(handle) =
                            runners.remove(&(subscriber_session_id, downstream_subscribe_id))
                        {
                            handle.abort();
                        }
                    }
                }
            }

            for (_, handle) in runners {
                handle.abort();
            }
        });

        Self {
            command_sender,
            command_runner,
        }
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<EgressCommand> {
        self.command_sender.clone()
    }

    async fn spawn_runner(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        cache_store: Arc<TrackCacheStore>,
        object_notify_producer_map: Arc<ObjectNotifyProducerMap>,
        request: EgressStartRequest,
    ) -> Option<JoinHandle<()>> {
        let publisher = session_repo
            .lock()
            .await
            .publisher(request.subscriber_session_id);
        let Some(publisher) = publisher else {
            tracing::error!("publisher session not found for egress start");
            return None;
        };

        let cache = cache_store.get_or_create(request.track_key);
        let latest_info_sender = object_notify_producer_map.get_or_create(request.track_key);

        let runner = EgressRunner::new(
            cache,
            latest_info_sender,
            publisher,
            request.published_resources,
        );

        Some(tokio::spawn(async move {
            if let Err(e) = runner.run().await {
                tracing::error!(?e, "egress runner finished with error");
            }
        }))
    }
}

impl Drop for EgressCoordinator {
    fn drop(&mut self) {
        self.command_runner.abort();
    }
}
