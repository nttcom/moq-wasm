use std::sync::Arc;
use tokio::sync::mpsc;

use crate::modules::{
    core::{
        data_receiver::receiver::DataReceiver, published_resource::PublishedResource,
        publisher::Publisher, subscription::Subscription,
    },
    event_resolver::stream_runner::StreamTaskRunner,
    relay::{
        cache::store::TrackCacheStore,
        egress::{reader::SubscriberAcceptReader, stream_allocator::StreamAllocator},
        ingest::receiver_registry::ReceiverRegistry,
        types::RelayTransport,
    },
    session_repository::SessionRepository,
    types::{SessionId, TrackKey, compose_session_track_key},
};

pub(crate) struct BindBySubscribeRequest {
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) subscription: Box<dyn Subscription>,
    pub(crate) publisher_session_id: SessionId,
    pub(crate) published_resources: PublishedResource,
}

enum IngestCommand {
    RegisterDataReceiver {
        track_key: TrackKey,
        receiver: DataReceiver,
    },
}

enum EgressCommand {
    StartReader {
        track_key: TrackKey,
        transport: RelayTransport,
        publisher: Box<dyn Publisher>,
        published_resources: PublishedResource,
    },
}

pub(crate) struct StreamBinder {
    command_sender: mpsc::Sender<BindBySubscribeRequest>,
    command_runner: tokio::task::JoinHandle<()>,
    ingest_runner: tokio::task::JoinHandle<()>,
    egress_runner: tokio::task::JoinHandle<()>,
}

impl StreamBinder {
    pub(crate) fn new(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        cache_store: Arc<TrackCacheStore>,
        receiver_registry: Arc<ReceiverRegistry>,
        stream_runner: Arc<StreamTaskRunner>,
    ) -> Self {

        let (ingest_sender, mut ingest_receiver) = mpsc::channel::<IngestCommand>(512);
        let receiver_registry_for_ingest = receiver_registry.clone();
        let ingest_runner = tokio::spawn(async move {
            while let Some(command) = ingest_receiver.recv().await {
                match command {
                    IngestCommand::RegisterDataReceiver {
                        track_key,
                        receiver,
                    } => {
                        receiver_registry_for_ingest.register_data_receiver(track_key, receiver);
                    }
                }
            }
        });

        let (egress_sender, mut egress_receiver) = mpsc::channel::<EgressCommand>(512);
        let cache_store_for_egress = cache_store.clone();
        let stream_runner_for_egress = stream_runner.clone();
        let egress_runner = tokio::spawn(async move {
            while let Some(command) = egress_receiver.recv().await {
                match command {
                    EgressCommand::StartReader {
                        track_key,
                        transport,
                        publisher,
                        published_resources,
                    } => {
                        let cache_store = cache_store_for_egress.clone();
                        let egress_task = async move {
                            let cache = cache_store.get_or_create(track_key);
                            let subscriber_track_alias = published_resources.track_alias();
                            let group_order = published_resources.group_order();
                            let filter_type = published_resources.filter_type();

                            let mut reader = SubscriberAcceptReader::new(
                                track_key,
                                cache,
                                &filter_type,
                                group_order,
                                transport,
                            )
                            .await;
                            let allocator =
                                StreamAllocator::new(publisher.as_ref(), &published_resources);
                            if let Err(error) = reader
                                .run_with_allocator(&allocator, subscriber_track_alias)
                                .await
                            {
                                tracing::error!(
                                    ?error,
                                    track_key,
                                    "egress relay loop finished with error"
                                );
                            }
                        };
                        stream_runner_for_egress
                            .add_task(Box::pin(egress_task))
                            .await;
                    }
                }
            }
        });

        let (command_sender, mut command_receiver) = mpsc::channel::<BindBySubscribeRequest>(512);
        let stream_runner_for_command = stream_runner.clone();
        let session_repo_for_command = session_repo.clone();
        let ingest_sender_for_command = ingest_sender.clone();
        let egress_sender_for_command = egress_sender.clone();
        let command_runner = tokio::spawn(async move {
            while let Some(request) = command_receiver.recv().await {
                let session_repo = session_repo_for_command.clone();
                let ingest_sender = ingest_sender_for_command.clone();
                let egress_sender = egress_sender_for_command.clone();
                let stream_runner = stream_runner_for_command.clone();
                let task = async move {
                    Self::handle_bind_request(
                        session_repo,
                        ingest_sender,
                        egress_sender,
                        stream_runner,
                        request,
                    )
                    .await;
                };
                stream_runner_for_command.add_task(Box::pin(task)).await;
            }
        });

        Self {
            command_sender,
            command_runner,
            ingest_runner,
            egress_runner,
        }
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<BindBySubscribeRequest> {
        self.command_sender.clone()
    }

    pub(crate) fn is_running(&self) -> bool {
        !self.command_runner.is_finished()
            && !self.ingest_runner.is_finished()
            && !self.egress_runner.is_finished()
    }

    async fn handle_bind_request(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        ingest_sender: mpsc::Sender<IngestCommand>,
        egress_sender: mpsc::Sender<EgressCommand>,
        stream_runner: Arc<StreamTaskRunner>,
        request: BindBySubscribeRequest,
    ) {
        tracing::info!("bind by subscribe");
        let publisher = session_repo
            .lock()
            .await
            .publisher(request.subscriber_session_id);
        let subscriber = session_repo
            .lock()
            .await
            .subscriber(request.publisher_session_id);
        if publisher.is_none() || subscriber.is_none() {
            tracing::error!("Publisher or Subscriber session not found.");
            return;
        }
        let (publisher, mut subscriber) = (publisher.unwrap(), subscriber.unwrap());
        let mut subscription = request.subscription;
        let receiver = match subscriber.create_data_receiver(subscription.as_mut()).await {
            Ok(receiver) => receiver,
            Err(error) => {
                tracing::error!(?error, "Failed to accept initial data receiver");
                return;
            }
        };

        let upstream_track_alias = receiver.get_track_alias();
        let track_key =
            compose_session_track_key(request.publisher_session_id, upstream_track_alias);
        let transport = match &receiver {
            DataReceiver::Datagram(_) => RelayTransport::Datagram,
            DataReceiver::Stream(_) => RelayTransport::Stream,
        };
        if let Err(error) = ingest_sender
            .send(IngestCommand::RegisterDataReceiver {
                track_key,
                receiver,
            })
            .await
        {
            tracing::error!(
                ?error,
                track_key,
                "Failed to dispatch initial ingest receiver"
            );
            return;
        }

        tracing::debug!(
            track_key,
            upstream_track_alias,
            ?transport,
            "relay ingest pipeline started"
        );

        if matches!(transport, RelayTransport::Stream) {
            let ingest_sender_for_streams = ingest_sender.clone();
            let stream_accept_task = async move {
                let mut subscription = subscription;
                loop {
                    let receiver =
                        match subscriber.create_data_receiver(subscription.as_mut()).await {
                            Ok(receiver) => receiver,
                            Err(error) => {
                                tracing::debug!(?error, track_key, "stream accept loop finished");
                                return;
                            }
                        };

                    match receiver {
                        DataReceiver::Stream(_) => {
                            if ingest_sender_for_streams
                                .send(IngestCommand::RegisterDataReceiver {
                                    track_key,
                                    receiver,
                                })
                                .await
                                .is_err()
                            {
                                tracing::error!(
                                    track_key,
                                    "Failed to dispatch stream receiver to ingest"
                                );
                                return;
                            }
                        }
                        DataReceiver::Datagram(_) => {
                            tracing::error!(
                                track_key,
                                "Expected stream receiver but got datagram receiver"
                            );
                            return;
                        }
                    }
                }
            };
            stream_runner.add_task(Box::pin(stream_accept_task)).await;
        }

        if let Err(error) = egress_sender
            .send(EgressCommand::StartReader {
                track_key,
                transport,
                publisher,
                published_resources: request.published_resources,
            })
            .await
        {
            tracing::error!(?error, track_key, "Failed to dispatch egress reader start");
        }
    }
}

impl Drop for StreamBinder {
    fn drop(&mut self) {
        self.command_runner.abort();
        self.ingest_runner.abort();
        self.egress_runner.abort();
    }
}
