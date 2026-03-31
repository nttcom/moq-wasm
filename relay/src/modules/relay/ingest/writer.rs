use std::sync::Arc;

use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::relay::{
    cache::store::TrackCacheStore,
    ingest::received_event::ReceivedEvent,
    types::{IngressTransportNotification, RelayTransport},
};

pub(crate) struct CacheWriter {
    sender: mpsc::Sender<ReceivedEvent>,
    join_handle: JoinHandle<()>,
}

impl CacheWriter {
    pub(crate) fn start(
        cache_store: Arc<TrackCacheStore>,
        transport_notifier: mpsc::Sender<IngressTransportNotification>,
        queue_capacity: usize,
    ) -> Self {
        let (sender, mut receiver) = mpsc::channel::<ReceivedEvent>(queue_capacity);
        let join_handle = tokio::spawn(async move {
            while let Some(event) = receiver.recv().await {
                match event {
                    ReceivedEvent::StreamOpened {
                        track_key,
                        group_id,
                        object,
                    } => {
                        let cache = cache_store.get_or_create(track_key);
                        cache.ensure_group(group_id).await;
                        if let Err(error) = transport_notifier
                            .send(IngressTransportNotification {
                                track_key,
                                transport: RelayTransport::Stream,
                            })
                            .await
                        {
                            tracing::debug!(?error, track_key, "failed to notify stream transport");
                        }
                        cache.append_object(track_key, group_id, object).await;
                    }
                    ReceivedEvent::DatagramOpened { track_key } => {
                        if let Err(error) = transport_notifier
                            .send(IngressTransportNotification {
                                track_key,
                                transport: RelayTransport::Datagram,
                            })
                            .await
                        {
                            tracing::debug!(
                                ?error,
                                track_key,
                                "failed to notify datagram transport"
                            );
                        }
                    }
                    ReceivedEvent::Object {
                        track_key,
                        group_id,
                        object,
                    } => {
                        let cache = cache_store.get_or_create(track_key);
                        cache.append_object(track_key, group_id, object).await;
                    }
                    ReceivedEvent::EndOfGroup {
                        track_key,
                        group_id,
                    } => {
                        let cache = cache_store.get_or_create(track_key);
                        cache.close_group(track_key, group_id).await;
                    }
                    ReceivedEvent::DatagramClosed { track_key } => {
                        tracing::debug!(track_key, "datagram reader closed");
                    }
                }
            }
        });

        Self {
            sender,
            join_handle,
        }
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<ReceivedEvent> {
        self.sender.clone()
    }
}

impl Drop for CacheWriter {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
