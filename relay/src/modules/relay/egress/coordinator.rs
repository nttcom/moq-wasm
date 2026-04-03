use std::sync::Arc;

use tokio::{sync::mpsc, task::JoinSet};

use crate::modules::{
    core::published_resource::PublishedResource,
    relay::{
        cache::store::TrackCacheStore,
        egress::runner::EgressRunner,
        notifications::{delivery_type_map::DeliveryTypeMap, sender_map::SenderMap},
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
}

impl EgressCoordinator {
    pub(crate) fn new(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        cache_store: Arc<TrackCacheStore>,
        sender_map: Arc<SenderMap>,
        delivery_type_map: Arc<DeliveryTypeMap>,
    ) -> Self {
        let (command_sender, mut command_receiver) = mpsc::channel::<EgressCommand>(512);

        let command_runner = tokio::spawn(async move {
            let mut joinset = JoinSet::new();
            loop {
                tokio::select! {
                    maybe_command = command_receiver.recv() => {
                        let Some(command) = maybe_command else { break };
                        match command {
                            EgressCommand::StartReader(request) => {
                                Self::spawn_runner(
                                    session_repo.clone(),
                                    cache_store.clone(),
                                    sender_map.clone(),
                                    delivery_type_map.clone(),
                                    request,
                                    &mut joinset,
                                )
                                .await;
                            }
                        }
                    }
                    Some(result) = joinset.join_next() => {
                        if let Err(e) = result {
                            tracing::error!("egress runner panicked: {:?}", e);
                        }
                    }
                }
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
        sender_map: Arc<SenderMap>,
        delivery_type_map: Arc<DeliveryTypeMap>,
        request: EgressStartRequest,
        joinset: &mut JoinSet<()>,
    ) {
        let publisher = session_repo
            .lock()
            .await
            .publisher(request.subscriber_session_id);
        let Some(publisher) = publisher else {
            tracing::error!("publisher session not found for egress start");
            return;
        };

        let cache = cache_store.get_or_create(request.track_key);
        let latest_info_sender = sender_map.get_or_create(request.track_key);

        let runner = EgressRunner::new(
            request.track_key,
            cache,
            latest_info_sender,
            delivery_type_map,
            publisher,
            request.published_resources,
        );

        joinset.spawn(async move {
            if let Err(e) = runner.run().await {
                tracing::error!(?e, "egress runner finished with error");
            }
        });
    }
}

impl Drop for EgressCoordinator {
    fn drop(&mut self) {
        self.command_runner.abort();
    }
}
