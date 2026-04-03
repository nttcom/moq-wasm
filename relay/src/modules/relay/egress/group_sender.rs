use std::sync::Arc;

use tokio::{sync::mpsc, task::JoinSet};

use crate::modules::{
    core::{
        data_sender::{DataSender, stream_sender_factory::StreamSenderFactory},
        published_resource::PublishedResource,
        publisher::Publisher,
    },
    relay::cache::track_cache::TrackCache,
};

use super::scheduler::GroupSendTask;

/// Receives `GroupSendTask` entries and spawns per-group send tasks.
pub(crate) struct GroupSender {
    cache: Arc<TrackCache>,
    publisher: Box<dyn Publisher>,
    published_resource: PublishedResource,
    receiver: mpsc::Receiver<GroupSendTask>,
}

impl GroupSender {
    pub(crate) fn new(
        cache: Arc<TrackCache>,
        publisher: Box<dyn Publisher>,
        published_resource: PublishedResource,
        receiver: mpsc::Receiver<GroupSendTask>,
    ) -> Self {
        Self {
            cache,
            publisher,
            published_resource,
            receiver,
        }
    }

    pub(crate) async fn run(mut self) {
        let mut stream_factory: Option<Box<dyn StreamSenderFactory>> = None;
        let mut joinset = JoinSet::<Option<u64>>::new();

        loop {
            tokio::select! {
                Some(req) = self.receiver.recv() => {
                    let sender = if req.is_stream {
                        let factory = stream_factory
                            .get_or_insert_with(|| self.publisher.new_stream_factory(&self.published_resource));
                        match factory.next().await {
                            Ok(s) => Some(s),
                            Err(e) => {
                                tracing::error!(?e, "failed to open stream sender");
                                None
                            }
                        }
                    } else {
                        Some(self.publisher.new_datagram(&self.published_resource))
                    };
                    if let Some(sender) = sender {
                        joinset.spawn(Self::send_task(
                            req.group_id,
                            req.start_offset,
                            self.cache.clone(),
                            sender,
                        ));
                    }
                }
                Some(result) = joinset.join_next() => {
                    if let Err(e) = result {
                        tracing::error!("egress send task panicked: {:?}", e);
                    }
                }
                else => break,
            }
        }
    }

    /// Sends all objects in a group (shared between stream and datagram).
    async fn send_task(
        group_id: u64,
        start_offset: u64,
        cache: Arc<TrackCache>,
        mut sender: Box<dyn DataSender>,
    ) -> Option<u64> {
        let mut next_index = start_offset;
        while let Some(object) = cache.get_object_or_wait(group_id, next_index).await {
            if sender.send_object((*object).clone()).await.is_err() {
                return Some(group_id);
            }
            next_index += 1;
        }
        Some(group_id)
    }
}
