use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use tokio::{
    sync::{Mutex, mpsc},
    task::JoinSet,
};
use tracing::{Instrument, Span};

use crate::modules::{
    core::{
        data_object::DataObject,
        data_sender::{DataSender, stream_sender_factory::StreamSenderFactory},
        publisher::Publisher,
        subscription::DownstreamSubscription,
    },
    relay::{cache::track_cache::TrackCache, types::SubgroupKey},
    types::TrackKey,
};

use super::scheduler::GroupSendTask;

type SharedStreamSenderFactory = Arc<Mutex<Box<dyn StreamSenderFactory>>>;

/// Receives `GroupSendTask` entries and spawns per-subgroup send tasks.
pub(crate) struct GroupSender {
    track_key: TrackKey,
    cache: Arc<TrackCache>,
    publisher: Arc<dyn Publisher>,
    downstream_subscription: DownstreamSubscription,
    receiver: mpsc::Receiver<GroupSendTask>,
    opened_stream_count: Arc<AtomicU64>,
}

struct StreamSendTask {
    track_alias: u64,
    group_id: u64,
    subgroup_id: u64,
    object_id: u64,
    track_key: TrackKey,
    cache: Arc<TrackCache>,
    factory: SharedStreamSenderFactory,
    opened_stream_count: Arc<AtomicU64>,
}

impl GroupSender {
    pub(crate) fn new(
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        publisher: Arc<dyn Publisher>,
        downstream_subscription: DownstreamSubscription,
        receiver: mpsc::Receiver<GroupSendTask>,
        opened_stream_count: Arc<AtomicU64>,
    ) -> Self {
        Self {
            track_key,
            cache,
            publisher,
            downstream_subscription,
            receiver,
            opened_stream_count,
        }
    }

    pub(crate) async fn run(mut self) {
        let mut stream_factory: Option<SharedStreamSenderFactory> = None;
        let mut joinset = JoinSet::<()>::new();
        let track_alias = self.downstream_subscription.track_alias();

        loop {
            tokio::select! {
                Some(task) = self.receiver.recv() => {
                    match task.key {
                        SubgroupKey::Stream { group_id, subgroup_id } => {
                            let factory = stream_factory
                                .get_or_insert_with(|| {
                                    Arc::new(Mutex::new(
                                        self.publisher.new_stream_factory(&self.downstream_subscription),
                                    ))
                                })
                                .clone();
                            let span = tracing::info_span!(
                                "relay.dataplane.egress.stream",
                                track_key = %self.track_key,
                                track_alias = track_alias,
                                group_id = group_id,
                                subgroup_id = subgroup_id,
                                object_id = task.object_id,
                                object_count = tracing::field::Empty,
                                end_reason = tracing::field::Empty,
                            );
                            joinset.spawn(
                                Self::send_stream_task(StreamSendTask {
                                    track_alias,
                                    group_id,
                                    subgroup_id,
                                    object_id: task.object_id,
                                    track_key: self.track_key.clone(),
                                    cache: self.cache.clone(),
                                    factory,
                                    opened_stream_count: self.opened_stream_count.clone(),
                                })
                                .instrument(span),
                            );
                        }
                        SubgroupKey::Datagram { .. } => {
                            let sender = self.publisher.new_datagram(&self.downstream_subscription);
                            joinset.spawn(Self::send_datagram_task(
                                track_alias,
                                task,
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

    async fn send_stream_task(task: StreamSendTask) {
        let span = Span::current();
        let key = SubgroupKey::Stream {
            group_id: task.group_id,
            subgroup_id: task.subgroup_id,
        };
        let Some(first) = task.cache.next_object_or_wait(key, task.object_id).await else {
            span.record("object_count", 0u64);
            span.record("end_reason", "no_objects");
            tracing::debug!("subgroup closed before any object to send");
            return;
        };

        // Opening the stream awaits peer stream credit; if the subscriber
        // stops granting streams every stream task queues on the factory,
        // so leave a trace before it.
        tracing::debug!("opening egress uni stream");
        let opened = {
            let mut factory = task.factory.lock().await;
            factory.next().await
        };
        let mut sender = match opened {
            Ok(sender) => sender,
            Err(e) => {
                span.record("object_count", 0u64);
                span.record("end_reason", "open_failed");
                tracing::error!(?e, "failed to open stream sender");
                return;
            }
        };
        task.opened_stream_count.fetch_add(1, Ordering::AcqRel);

        // The header is regenerated from the object's canonical properties;
        // extensions are always declared present so no later object can lose
        // its extension headers on the wire.
        let header = moqt::SubgroupHeader::new(
            task.track_alias,
            task.group_id,
            moqt::SubgroupId::Value(task.subgroup_id),
            first.publisher_priority,
            true,
            false,
        );
        let message_type = header.message_type;
        tracing::debug!(track_key = %task.track_key, "egress sending subgroup header");
        if let Err(error) = sender.send_object(DataObject::SubgroupHeader(header)).await {
            span.record("object_count", 0u64);
            span.record("end_reason", "send_header_failed");
            tracing::error!(?error, track_key = %task.track_key, "failed to send subgroup header");
            return;
        }

        let mut object_count = 0u64;
        let mut prev_sent_object_id = None;
        let mut next = Some(first);
        while let Some(object) = next {
            let object_id = object.location.object_id;
            tracing::debug!(track_key = %task.track_key, object_id, "egress sending subgroup object");
            let field = object.to_subgroup_object_field(message_type, prev_sent_object_id);
            if sender
                .send_object(DataObject::SubgroupObject(field))
                .await
                .is_err()
            {
                span.record("object_count", object_count);
                span.record("end_reason", "send_object_failed");
                tracing::error!(track_key = %task.track_key, object_id, "failed to send subgroup object");
                return;
            }
            object_count += 1;
            prev_sent_object_id = Some(object_id);
            next = task
                .cache
                .next_object_or_wait(key, object_id.saturating_add(1))
                .await;
        }
        span.record("object_count", object_count);
        span.record("end_reason", "cache_closed");
        if let Err(error) = sender.close().await {
            tracing::warn!(?error, track_key = %task.track_key, "failed to close egress stream sender");
        }
    }

    async fn send_datagram_task(
        track_alias: u64,
        task: GroupSendTask,
        cache: Arc<TrackCache>,
        mut sender: Box<dyn DataSender>,
    ) {
        let mut cursor = task.object_id;
        while let Some(object) = cache.next_object_or_wait(task.key, cursor).await {
            let object_id = object.location.object_id;
            tracing::debug!(
                track_alias,
                group_id = task.key.group_id(),
                object_id,
                "egress sending datagram object"
            );
            let datagram = object.to_object_datagram(track_alias);
            if sender
                .send_object(DataObject::ObjectDatagram(datagram))
                .await
                .is_err()
            {
                return;
            }
            cursor = object_id.saturating_add(1);
        }
    }
}
