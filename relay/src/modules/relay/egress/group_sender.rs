use std::sync::Arc;

use tokio::{sync::mpsc, task::JoinSet};
use tracing::{Instrument, Span};

use crate::modules::{
    core::{
        data_sender::{DataSender, stream_sender_factory::StreamSenderFactory},
        published_resource::PublishedResource,
        publisher::Publisher,
    },
    relay::{cache::track_cache::TrackCache, types::StreamSubgroupId},
    types::TrackKey,
};

use super::scheduler::GroupSendTask;

/// Receives `GroupSendTask` entries and spawns per-group send tasks.
pub(crate) struct GroupSender {
    track_key: TrackKey,
    cache: Arc<TrackCache>,
    publisher: Box<dyn Publisher>,
    published_resource: PublishedResource,
    receiver: mpsc::Receiver<GroupSendTask>,
}

impl GroupSender {
    pub(crate) fn new(
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        publisher: Box<dyn Publisher>,
        published_resource: PublishedResource,
        receiver: mpsc::Receiver<GroupSendTask>,
    ) -> Self {
        Self {
            track_key,
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
                            let span = tracing::info_span!(
                                "relay.dataplane.egress.stream",
                                track_key = self.track_key,
                                track_alias = track_alias,
                                group_id = group_id,
                                subgroup_id = tracing::field::debug(&subgroup_id),
                                start_offset = start_offset,
                                object_count = tracing::field::Empty,
                                end_reason = tracing::field::Empty,
                            );
                            match async { factory.next().await }.instrument(span.clone()).await {
                                Ok(sender) => {
                                    joinset.spawn(Self::send_stream_task(
                                        track_alias,
                                        group_id,
                                        subgroup_id,
                                        start_offset,
                                        self.track_key,
                                        self.cache.clone(),
                                        sender,
                                    ).instrument(span));
                                }
                                Err(e) => {
                                    span.record("object_count", 0u64);
                                    span.record("end_reason", "open_failed");
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
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        mut sender: Box<dyn DataSender>,
    ) {
        let span = Span::current();
        let mut object_count = 0u64;
        let Some(header) = cache
            .get_stream_object_or_wait(group_id, &subgroup_id, 0)
            .await
        else {
            span.record("object_count", object_count);
            span.record("end_reason", "header_unavailable");
            tracing::warn!(
                track_key,
                track_alias,
                group_id,
                subgroup_id = ?subgroup_id,
                "stream egress task ended before subgroup header became available"
            );
            return;
        };
        tracing::info!(
            track_key,
            track_alias,
            group_id,
            subgroup_id = ?subgroup_id,
            "egress sending subgroup header"
        );
        if let Err(error) = sender.send_object((*header).clone()).await {
            span.record("object_count", object_count);
            span.record("end_reason", "send_header_failed");
            tracing::error!(
                ?error,
                track_key,
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
            tracing::debug!(
                track_key,
                track_alias,
                group_id,
                subgroup_id = ?subgroup_id,
                object_id_delta,
                cache_index = next_index,
                "egress sending subgroup object"
            );
            if sender.send_object((*object).clone()).await.is_err() {
                span.record("object_count", object_count);
                span.record("end_reason", "send_object_failed");
                tracing::error!(
                    track_key,
                    track_alias,
                    group_id,
                    subgroup_id = ?subgroup_id,
                    object_index = next_index,
                    "failed to send subgroup object"
                );
                return;
            }
            object_count += 1;
            next_index += 1;
        }
        span.record("object_count", object_count);
        span.record("end_reason", "cache_closed");
        if let Err(error) = sender.close().await {
            tracing::warn!(
                ?error,
                track_key,
                track_alias,
                group_id,
                subgroup_id = ?subgroup_id,
                "failed to close egress stream sender"
            );
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
