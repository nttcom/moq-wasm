use std::sync::Arc;

use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::relay::ingest::received_event::ReceivedEvent;

pub(crate) struct CacheWriter {
    sender: mpsc::Sender<ReceivedEvent>,
    join_handle: JoinHandle<()>,
}

impl CacheWriter {
    pub(crate) fn start(
        sender_map: Arc<SenderMap>,
        cache: Arc<dyn Cache>,
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
                        let cache = cache.add_object(track_key, group_id, object).await;
                    }
                    ReceivedEvent::DatagramOpened {
                        track_key,
                        group_id,
                    } => {
                        let cache = cache.add_object(track_key, group_id).await;
                    }
                    ReceivedEvent::Object {
                        track_key,
                        group_id,
                        object,
                    } => {
                        let cache = cache.add_object(track_key, group_id, object).await;
                    }
                    ReceivedEvent::EndOfGroup {
                        track_key,
                        group_id,
                    } => {
                        
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
