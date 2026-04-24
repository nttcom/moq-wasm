use std::sync::Arc;

use tokio::{sync::mpsc, task::JoinSet};

use crate::modules::{
    core::{
        data_sender::{DataSender, stream_sender_factory::StreamSenderFactory},
        published_resource::PublishedResource,
        publisher::Publisher,
    },
    relay::{cache::track_cache::TrackCache, types::StreamSubgroupId},
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
        let mut joinset = JoinSet::<()>::new();
        let track_alias = self.published_resource.track_alias();

        loop {
            tokio::select! {
                Some(req) = self.receiver.recv() => {
                    match req {
                        GroupSendTask::Stream {
                            group_id,
                            subgroup_id,
                            start_offset,
                        } => {
                            let factory = stream_factory
                                .get_or_insert_with(|| self.publisher.new_stream_factory(&self.published_resource));
                            match factory.next().await {
                                Ok(sender) => {
                                    joinset.spawn(Self::send_stream_task(
                                        track_alias,
                                        group_id,
                                        subgroup_id,
                                        start_offset,
                                        self.cache.clone(),
                                        sender,
                                    ));
                                }
                                Err(e) => {
                                    tracing::error!(?e, "failed to open stream sender");
                                }
                            }
                        }
                        GroupSendTask::Datagram {
                            group_id,
                            start_offset,
                        } => {
                            let sender = self.publisher.new_datagram(&self.published_resource);
                            joinset.spawn(Self::send_datagram_task(
                                track_alias,
                                group_id,
                                start_offset,
                                self.cache.clone(),
                                sender,
                            ));
                        }
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

    async fn send_stream_task(
        track_alias: u64,
        group_id: u64,
        subgroup_id: StreamSubgroupId,
        start_offset: u64,
        cache: Arc<TrackCache>,
        mut sender: Box<dyn DataSender>,
    ) {
        let Some(header) = cache
            .get_stream_object_or_wait(group_id, &subgroup_id, 0)
            .await
        else {
            tracing::warn!(
                track_alias,
                group_id,
                subgroup_id = ?subgroup_id,
                "stream egress task ended before subgroup header became available"
            );
            return;
        };
        tracing::info!(
            track_alias,
            group_id,
            subgroup_id = ?subgroup_id,
            "egress sending subgroup header"
        );
        if let Err(error) = sender.send_object((*header).clone()).await {
            tracing::error!(
                ?error,
                track_alias,
                group_id,
                subgroup_id = ?subgroup_id,
                "failed to send subgroup header"
            );
            return;
        }

        let mut next_index = start_offset.max(1);
        while let Some(object) = cache
            .get_stream_object_or_wait(group_id, &subgroup_id, next_index)
            .await
        {
            let object_id_delta = match &*object {
                crate::modules::core::data_object::DataObject::SubgroupObject(field) => {
                    Some(field.object_id_delta)
                }
                _ => None,
            };
            tracing::info!(
                track_alias,
                group_id,
                subgroup_id = ?subgroup_id,
                object_id_delta,
                cache_index = next_index,
                "egress sending subgroup object"
            );
            if sender.send_object((*object).clone()).await.is_err() {
                tracing::error!(
                    track_alias,
                    group_id,
                    subgroup_id = ?subgroup_id,
                    object_index = next_index,
                    "failed to send subgroup object"
                );
                return;
            }
            next_index += 1;
        }
    }

    async fn send_datagram_task(
        track_alias: u64,
        group_id: u64,
        start_offset: u64,
        cache: Arc<TrackCache>,
        mut sender: Box<dyn DataSender>,
    ) {
        let mut next_index = start_offset;
        while let Some(object) = cache
            .get_datagram_object_or_wait(group_id, next_index)
            .await
        {
            let datagram_object_id = match &*object {
                crate::modules::core::data_object::DataObject::ObjectDatagram(datagram) => {
                    datagram.field.object_id()
                }
                _ => None,
            };
            tracing::info!(
                track_alias,
                group_id,
                object_id = datagram_object_id,
                cache_index = next_index,
                "egress sending datagram object"
            );
            if sender.send_object((*object).clone()).await.is_err() {
                return;
            }
            next_index += 1;
        }
    }
}
