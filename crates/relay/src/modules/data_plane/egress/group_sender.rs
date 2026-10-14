use moqt::object::DataStreamResetCode;
use std::sync::Arc;

use tokio::{
    sync::{Mutex, mpsc, watch},
    task::JoinSet,
};
use tracing::{Instrument, Span};

use crate::modules::{
    data_plane::cache::{
        subgroup_key::SubgroupKey,
        track_cache::{NextObject, TrackCache, TrackMalformed},
    },
    domain::{
        delivery_control::DeliveryControl, delivery_stats::DeliveryStats, track_key::TrackKey,
    },
    session::{
        data_object::DataObject,
        data_sender::{DataSender, stream_sender_factory::StreamSenderFactory},
        publisher::Publisher,
        subscription::DownstreamSubscription,
    },
};

use super::{group_sequence::GroupSequence, scheduler::GroupSendTask};

type SharedStreamSenderFactory = Arc<Mutex<Box<dyn StreamSenderFactory>>>;

pub(crate) struct GroupSender {
    track_key: TrackKey,
    cache: Arc<TrackCache>,
    publisher: Arc<dyn Publisher>,
    downstream_subscription: DownstreamSubscription,
    receiver: mpsc::Receiver<GroupSendTask>,
    delivery_stats: Arc<DeliveryStats>,
    delivery_control_receiver: watch::Receiver<DeliveryControl>,
    group_sequence: GroupSequence,
}

/// Where a send task reads its subgroup from: the cache, until a
/// SUBSCRIBE_UPDATE moves the subscription's range past the group.
struct SubgroupSource {
    key: SubgroupKey,
    generation: usize,
    cache: Arc<TrackCache>,
    delivery_control_receiver: watch::Receiver<DeliveryControl>,
}

enum SubgroupNext {
    Cache(NextObject),
    Malformed,
    /// draft-14 §10.4.3: the group left the subscription's range, so an open
    /// stream is reset.
    LeftRange,
}

impl SubgroupSource {
    async fn next(&mut self, object_id: u64) -> SubgroupNext {
        let group_id = self.key.group_id();
        let receiver = &mut self.delivery_control_receiver;
        let left_range = async {
            let left = receiver
                .wait_for(|control| !control.contains_group(group_id))
                .await
                .is_ok();
            if !left {
                // Without its registration the range can no longer change.
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            biased;
            () = left_range => SubgroupNext::LeftRange,
            next = self.cache.next_subgroup_object_or_wait(self.key, self.generation, object_id) => {
                match next {
                    Ok(next) => SubgroupNext::Cache(next),
                    Err(TrackMalformed) => SubgroupNext::Malformed,
                }
            }
        }
    }
}

struct StreamSendTask {
    source: SubgroupSource,
    object_id: u64,
    factory: SharedStreamSenderFactory,
    delivery_stats: Arc<DeliveryStats>,
    subscriber_priority: u8,
    group_order: moqt::object::GroupOrder,
    group_sequence: u64,
}

impl GroupSender {
    pub(crate) fn new(
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        publisher: Arc<dyn Publisher>,
        downstream_subscription: DownstreamSubscription,
        receiver: mpsc::Receiver<GroupSendTask>,
        delivery_stats: Arc<DeliveryStats>,
        delivery_control_receiver: watch::Receiver<DeliveryControl>,
    ) -> Self {
        Self {
            track_key,
            cache,
            publisher,
            downstream_subscription,
            receiver,
            delivery_stats,
            delivery_control_receiver,
            group_sequence: GroupSequence::new(),
        }
    }

    fn source(&self, task: &GroupSendTask) -> SubgroupSource {
        SubgroupSource {
            key: task.key,
            generation: task.generation,
            cache: self.cache.clone(),
            delivery_control_receiver: self.delivery_control_receiver.clone(),
        }
    }

    pub(crate) async fn run(mut self) {
        let mut stream_factory: Option<SharedStreamSenderFactory> = None;
        let mut joinset = JoinSet::<()>::new();
        let track_alias = self.downstream_subscription.track_alias();
        let subscriber_priority = self.downstream_subscription.subscriber_priority();
        let group_order = self.downstream_subscription.group_order();

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
                            let group_sequence = self.group_sequence.sequence_of(group_id);
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
                                    source: self.source(&task),
                                    object_id: task.object_id,
                                    factory,
                                    delivery_stats: self.delivery_stats.clone(),
                                    subscriber_priority,
                                    group_order,
                                    group_sequence,
                                })
                                .instrument(span),
                            );
                        }
                        SubgroupKey::Datagram { .. } => {
                            let sender = self.publisher.new_datagram(&self.downstream_subscription);
                            joinset.spawn(Self::send_datagram_task(
                                track_alias,
                                self.source(&task),
                                task.object_id,
                                sender,
                                self.delivery_stats.clone(),
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
        let (object_count, end_reason) = Self::send_stream(task).await;
        let span = Span::current();
        span.record("object_count", object_count);
        span.record("end_reason", end_reason);
    }

    async fn send_stream(mut task: StreamSendTask) -> (u64, &'static str) {
        let SubgroupKey::Stream {
            group_id,
            subgroup_id,
        } = task.source.key
        else {
            unreachable!("run() spawns send_stream_task only for SubgroupKey::Stream");
        };
        let first = match task.source.next(task.object_id).await {
            SubgroupNext::Cache(NextObject::Object(first)) => first,
            SubgroupNext::LeftRange => {
                tracing::debug!("group left the subscription range before any object to send");
                return (0, "left_range");
            }
            SubgroupNext::Cache(_) | SubgroupNext::Malformed => {
                tracing::debug!("subgroup ended before any object to send");
                return (0, "no_objects");
            }
        };

        let priority = moqt::object::StreamPriority {
            subscriber_priority: task.subscriber_priority,
            publisher_priority: first.object.publisher_priority,
            group_order: task.group_order,
            group_sequence: task.group_sequence,
            subgroup_id,
        };
        // Opening the stream awaits peer stream credit; if the subscriber
        // stops granting streams every stream task queues on the factory,
        // so leave a trace before it.
        tracing::debug!(
            group_sequence = task.group_sequence,
            transport_priority = priority.transport_priority(),
            "opening egress uni stream"
        );
        let opened = {
            let mut factory = task.factory.lock().await;
            factory.next(priority).await
        };
        let mut sender = match opened {
            Ok(sender) => sender,
            Err(e) => {
                tracing::error!(?e, "failed to open stream sender");
                return (0, "open_failed");
            }
        };
        task.delivery_stats.record_stream_opened();

        let start = moqt::object::SubgroupStart {
            group_id,
            subgroup_id,
            publisher_priority: first.object.publisher_priority,
            ends_group: false,
        };
        tracing::debug!("egress sending subgroup header");
        if let Err(error) = sender.send_object(DataObject::SubgroupHeader(start)).await {
            tracing::error!(?error, "failed to send subgroup header");
            return (0, "send_header_failed");
        }

        let mut object_count = 0u64;
        let mut next = NextObject::Object(first);
        while let NextObject::Object(object) = next {
            let object_id = object.object.location.object_id;
            match object.load().await {
                Ok(loaded) => {
                    tracing::debug!(object_id, "egress sending subgroup object");
                    if sender
                        .send_object(DataObject::SubgroupObject(loaded))
                        .await
                        .is_err()
                    {
                        tracing::error!(object_id, "failed to send subgroup object");
                        return (object_count, "send_object_failed");
                    }
                    task.delivery_stats
                        .record_object_sent(object.payload_len(), object.received_at);
                    object_count += 1;
                }
                // Its payload could not be read: skipped, as an expired object is.
                Err(error) => tracing::debug!(object_id, ?error, "skipping unloadable object"),
            }
            next = match task.source.next(object_id.saturating_add(1)).await {
                SubgroupNext::Cache(next) => next,
                // The runner terminates the subscription with PUBLISH_DONE; a FIN
                // here would claim the partial subgroup is complete.
                SubgroupNext::Malformed => return (object_count, "malformed_track"),
                SubgroupNext::LeftRange => {
                    task.delivery_stats.record_stream_reset();
                    if let Err(error) = sender.reset(DataStreamResetCode::Cancelled).await {
                        tracing::warn!(?error, "failed to reset egress stream sender");
                    }
                    return (object_count, "left_range");
                }
            };
        }
        if matches!(next, NextObject::Aborted) {
            // draft-14 §10.4.3: a subgroup that ended upstream without a FIN
            // may be missing objects, so the downstream stream is reset.
            task.delivery_stats.record_stream_reset();
            if let Err(error) = sender.reset(DataStreamResetCode::InternalError).await {
                tracing::warn!(?error, "failed to reset egress stream sender");
            }
            (object_count, "upstream_aborted")
        } else {
            if let Err(error) = sender.close().await {
                tracing::warn!(?error, "failed to close egress stream sender");
            }
            (object_count, "cache_closed")
        }
    }

    async fn send_datagram_task(
        track_alias: u64,
        mut source: SubgroupSource,
        object_id: u64,
        mut sender: Box<dyn DataSender>,
        delivery_stats: Arc<DeliveryStats>,
    ) {
        let mut cursor = object_id;
        while let SubgroupNext::Cache(NextObject::Object(object)) = source.next(cursor).await {
            let object_id = object.object.location.object_id;
            cursor = object_id.saturating_add(1);
            let loaded = match object.load().await {
                Ok(loaded) => loaded,
                Err(error) => {
                    tracing::debug!(object_id, ?error, "skipping unloadable object");
                    continue;
                }
            };
            tracing::debug!(
                track_alias,
                group_id = source.key.group_id(),
                object_id,
                "egress sending datagram object"
            );
            if sender
                .send_object(DataObject::Datagram(loaded))
                .await
                .is_err()
            {
                return;
            }
            delivery_stats.record_object_sent(object.payload_len(), object.received_at);
        }
    }
}
