use std::sync::Arc;

use tokio::{sync::mpsc, task::JoinSet};
use tracing::{Instrument, Span};

use crate::modules::{
    core::{
        data_sender::{DataSender, stream_sender_factory::StreamSenderFactory},
        publisher::Publisher,
        subscription::DownstreamSubscription,
    },
    relay::{cache::track_cache::TrackCache, types::StreamSubgroupId},
    types::TrackKey,
};

use super::scheduler::GroupSendTask;

fn stream_object_id_to_cache_index(object_id: u64) -> u64 {
    object_id + 1
}

/// Receives `GroupSendTask` entries and spawns per-group send tasks.
pub(crate) struct GroupSender {
    track_key: TrackKey,
    cache: Arc<TrackCache>,
    publisher: Box<dyn Publisher>,
    downstream_subscription: DownstreamSubscription,
    receiver: mpsc::Receiver<GroupSendTask>,
}

impl GroupSender {
    pub(crate) fn new(
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        publisher: Box<dyn Publisher>,
        downstream_subscription: DownstreamSubscription,
        receiver: mpsc::Receiver<GroupSendTask>,
    ) -> Self {
        Self {
            track_key,
            cache,
            publisher,
            downstream_subscription,
            receiver,
        }
    }

    pub(crate) async fn run(mut self) {
        let mut stream_factory: Option<Box<dyn StreamSenderFactory>> = None;
        let mut joinset = JoinSet::<()>::new();
        let track_alias = self.downstream_subscription.track_alias();

        loop {
            tokio::select! {
                Some(req) = self.receiver.recv() => {
                    match req {
                        GroupSendTask::Stream {
                            group_id,
                            subgroup_id,
                            object_id,
                        } => {
                            let factory = stream_factory
                                .get_or_insert_with(|| self.publisher.new_stream_factory(&self.downstream_subscription));
                            let span = tracing::info_span!(
                                "relay.dataplane.egress.stream",
                                track_key = %self.track_key,
                                track_alias = track_alias,
                                group_id = group_id,
                                subgroup_id = tracing::field::debug(&subgroup_id),
                                object_id = object_id,
                                object_count = tracing::field::Empty,
                                end_reason = tracing::field::Empty,
                            );
                            // Opening the stream awaits peer stream credit; if the
                            // subscriber stops granting streams this blocks the
                            // whole GroupSender, so leave a trace before it.
                            match async {
                                tracing::debug!("opening egress uni stream");
                                factory.next().await
                            }
                            .instrument(span.clone())
                            .await
                            {
                                Ok(sender) => {
                                    joinset.spawn(Self::send_stream_task(
                                        track_alias,
                                        group_id,
                                        subgroup_id,
                                        object_id,
                                        self.track_key.clone(),
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
                            object_id,
                        } => {
                            let sender = self.publisher.new_datagram(&self.downstream_subscription);
                            joinset.spawn(Self::send_datagram_task(
                                track_alias,
                                group_id,
                                object_id,
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
        object_id: u64,
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
                track_key = %track_key,
                track_alias,
                group_id,
                subgroup_id = ?subgroup_id,
                "stream egress task ended before subgroup header became available"
            );
            return;
        };
        tracing::debug!(
            track_key = %track_key,
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
                track_key = %track_key,
                track_alias,
                group_id,
                subgroup_id = ?subgroup_id,
                "failed to send subgroup header"
            );
            return;
        }

        let mut next_index = stream_object_id_to_cache_index(object_id);
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
                track_key = %track_key,
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
                    track_key = %track_key,
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
                track_key = %track_key,
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
        object_id: u64,
        cache: Arc<TrackCache>,
        mut sender: Box<dyn DataSender>,
    ) {
        let mut next_index = object_id;
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
            tracing::debug!(
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
