use std::sync::Arc;

use crate::modules::{
    core::{
        data_receiver::receiver::DataReceiver, published_resource::PublishedResource,
        subscription::Subscription,
    },
    event_resolver::stream_runner::StreamTaskRunner,
    relay::{
        cache::store::TrackCacheStore,
        egress::{reader::SubscriberAcceptReader, stream_allocator::StreamAllocator},
        ingest::{receiver_registry::ReceiverRegistry, writer::CacheWriter},
        types::RelayTransport,
    },
    session_repository::SessionRepository,
    types::{SessionId, compose_session_track_key},
};

pub(crate) struct StreamBinder {
    session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    stream_runner: StreamTaskRunner,
}

impl StreamBinder {
    pub(crate) fn new(session_repo: Arc<tokio::sync::Mutex<SessionRepository>>) -> Self {
        Self {
            session_repo,
            stream_runner: StreamTaskRunner::new(),
        }
    }

    pub(crate) async fn bind_by_subscribe(
        &self,
        subscriber_session_id: SessionId,
        subscription: Box<dyn Subscription>,
        publisher_session_id: SessionId,
        published_resources: PublishedResource,
    ) {
        tracing::info!("bind by subscribe");
        let publisher = self
            .session_repo
            .lock()
            .await
            .publisher(subscriber_session_id);
        let subscriber = self
            .session_repo
            .lock()
            .await
            .subscriber(publisher_session_id);
        if publisher.is_none() || subscriber.is_none() {
            tracing::error!("Publisher or Subscriber session not found.");
            return;
        }
        let (publisher, mut subscriber) = (publisher.unwrap(), subscriber.unwrap());
        let task = async move {
            let mut subscription = subscription;
            let receiver = match subscriber.create_data_receiver(subscription.as_mut()).await {
                Ok(receiver) => receiver,
                Err(error) => {
                    tracing::error!(?error, "Failed to accept initial data receiver");
                    return;
                }
            };

            let upstream_track_alias = receiver.get_track_alias();
            let track_key = compose_session_track_key(publisher_session_id, upstream_track_alias);
            let transport = match &receiver {
                DataReceiver::Datagram(_) => RelayTransport::Datagram,
                DataReceiver::Stream(_) => RelayTransport::Stream,
            };

            let cache_store = Arc::new(TrackCacheStore::new());
            let cache_writer = CacheWriter::start(cache_store.clone(), 1024);
            let registry = Arc::new(ReceiverRegistry::new(cache_writer.sender()));
            registry.register_data_receiver(track_key, receiver);

            tracing::debug!(
                track_key,
                upstream_track_alias,
                ?transport,
                "relay ingest pipeline started"
            );

            match transport {
                RelayTransport::Stream => {
                    let registry_for_streams = registry.clone();
                    tokio::spawn(async move {
                        loop {
                            let receiver =
                                match subscriber.create_data_receiver(subscription.as_mut()).await {
                                    Ok(receiver) => receiver,
                                    Err(error) => {
                                        tracing::debug!(
                                            ?error,
                                            track_key,
                                            "stream accept loop finished"
                                        );
                                        return;
                                    }
                                };

                            match receiver {
                                DataReceiver::Stream(_) => {
                                    registry_for_streams.register_data_receiver(track_key, receiver)
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
                    });
                }
                RelayTransport::Datagram => {}
            }

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
            let allocator = StreamAllocator::new(publisher.as_ref(), &published_resources);
            if let Err(error) = reader
                .run_with_allocator(&allocator, subscriber_track_alias)
                .await
            {
                tracing::error!(?error, track_key, "egress relay loop finished with error");
            }
        };
        self.stream_runner.add_task(Box::pin(task)).await;
    }
}
