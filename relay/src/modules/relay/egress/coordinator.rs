use std::{collections::HashMap, sync::Arc};

use tokio::{sync::mpsc, task::JoinHandle};
use tracing::{Instrument, Span};

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
    pub(crate) track_namespace: String,
    pub(crate) track_name: String,
    pub(crate) published_resources: PublishedResource,
    pub(crate) parent_span: Span,
}

pub(crate) struct EgressFetchRequest {
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) request_id: u64,
    pub(crate) track_key: TrackKey,
    pub(crate) start_location: moqt::Location,
    pub(crate) end_location: moqt::Location,
}

pub(crate) enum EgressCommand {
    StartReader(Box<EgressStartRequest>),
    StopReader {
        subscriber_session_id: SessionId,
        downstream_subscribe_id: u64,
    },
    StartFetch(EgressFetchRequest),
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
                        let request = *request;
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
                    EgressCommand::StartFetch(request) => {
                        Self::spawn_fetch_delivery(
                            session_repo.clone(),
                            cache_store.clone(),
                            request,
                        )
                        .await;
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

    async fn spawn_fetch_delivery(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        cache_store: Arc<TrackCacheStore>,
        request: EgressFetchRequest,
    ) {
        // publisher = relay's outbound handle to the session that issued FETCH
        let publisher = session_repo
            .lock()
            .await
            .publisher(request.subscriber_session_id);
        let Some(publisher) = publisher else {
            tracing::error!("session not found for fetch");
            return;
        };

        let Some(cache) = cache_store.get(request.track_key) else {
            tracing::error!("cache not found for fetch");
            return;
        };

        tokio::spawn(async move {
            let sender = match publisher.new_fetch_sender(request.request_id).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(?e, "failed to create fetch sender");
                    return;
                }
            };
            let objects = cache
                .get_fetch_objects(request.start_location, request.end_location)
                .await;
            for object in objects {
                if let Err(e) = sender.send(object).await {
                    tracing::error!(?e, "failed to send fetch object");
                    return;
                }
            }
            if let Err(e) = sender.close().await {
                tracing::error!(?e, "failed to close fetch stream");
            }
        });
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
            tracing::error!("subscriber session not found for egress start");
            return None;
        };

        let cache = cache_store.get_or_create(request.track_key);
        let latest_info_sender = object_notify_producer_map.get_or_create(request.track_key);
        let track_alias = request.published_resources.track_alias();
        let egress_track_span = tracing::info_span!(
            parent: &request.parent_span,
            "relay.dataplane.egress.track",
            subscriber_session_id = %request.subscriber_session_id,
            downstream_subscribe_id = request.downstream_subscribe_id,
            track_key = request.track_key,
            track_alias = track_alias,
            track_namespace = %request.track_namespace,
            track_name = %request.track_name,
        );

        let runner = EgressRunner::new(
            request.track_key,
            cache,
            latest_info_sender,
            publisher,
            request.published_resources,
        );

        Some(tokio::spawn(
            async move {
                if let Err(e) = runner.run().await {
                    tracing::error!(?e, "egress runner finished with error");
                }
            }
            .instrument(egress_track_span),
        ))
    }
}

impl Drop for EgressCoordinator {
    fn drop(&mut self) {
        self.command_runner.abort();
    }
}
