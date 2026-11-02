use moqt::{TransportSendError, object::DataStreamResetCode};
use std::sync::Arc;

use tokio::{
    sync::{Mutex, mpsc, watch},
    task::JoinSet,
};
use tracing::{Instrument, Span};

use crate::modules::{
    data_plane::cache::{
        subgroup_key::SubgroupKey,
        track_cache::{NextObject, PartialEnd, PartialObject, TrackCache, TrackMalformed},
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
                            let span = tracing::debug_span!(
                                "relay.dataplane.egress.stream",
                                track_namespace = %self.track_key.track_namespace, track_name = %self.track_key.track_name,
                                track_alias = track_alias,
                                group_id = group_id,
                                subgroup_id = subgroup_id,
                                object_id = task.object_id,
                                object_count = tracing::field::Empty,
                                end_reason = tracing::field::Empty,
                            );
                            // Also instrument with the track span, so the task stays inside it
                            // when the debug stream span is disabled.
                            joinset.spawn(
                                Self::send_stream_task(
                                    StreamSendTask {
                                        source: self.source(&task),
                                        object_id: task.object_id,
                                        factory,
                                        delivery_stats: self.delivery_stats.clone(),
                                        subscriber_priority,
                                        group_order,
                                        group_sequence,
                                    },
                                    span.clone(),
                                )
                                .instrument(span)
                                .instrument(Span::current()),
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
                        tracing::error!(error = ?e, "egress send task panicked");
                    }
                }
                else => break,
            }
        }
    }

    async fn send_stream_task(task: StreamSendTask, span: Span) {
        let (object_count, end_reason) = Self::send_stream(task).await;
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
        let (first, publisher_priority) = match task.source.next(task.object_id).await {
            SubgroupNext::Cache(NextObject::Object(first)) => {
                let publisher_priority = first.object.publisher_priority;
                (NextObject::Object(first), publisher_priority)
            }
            SubgroupNext::Cache(NextObject::Partial(first)) => {
                let publisher_priority = first.object.publisher_priority;
                (NextObject::Partial(first), publisher_priority)
            }
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
            publisher_priority,
            group_order: task.group_order,
            group_sequence: task.group_sequence,
            subgroup_id,
        };
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
                tracing::error!(error = ?e, "failed to open stream sender");
                return (0, "open_failed");
            }
        };
        task.delivery_stats.record_stream_opened();

        let start = moqt::object::SubgroupStart {
            group_id,
            subgroup_id,
            publisher_priority,
            ends_group: false,
        };
        tracing::debug!("egress sending subgroup header");
        if let Err(error) = sender.send_object(DataObject::SubgroupHeader(start)).await {
            return (0, send_failure_reason(&error, None, "send_header_failed"));
        }

        let mut object_count = 0u64;
        let mut next = first;
        loop {
            let object_id = match next {
                NextObject::Object(object) => {
                    let object_id = object.object.location.object_id;
                    match object.load().await {
                        Ok(loaded) => {
                            tracing::debug!(object_id, "egress sending subgroup object");
                            if let Err(error) =
                                sender.send_object(DataObject::SubgroupObject(loaded)).await
                            {
                                let reason = send_failure_reason(
                                    &error,
                                    Some(object_id),
                                    "send_object_failed",
                                );
                                return (object_count, reason);
                            }
                            task.delivery_stats
                                .record_object_sent(object.payload_len(), object.received_at);
                            object_count += 1;
                        }
                        Err(error) => {
                            tracing::debug!(object_id, ?error, "skipping unloadable object")
                        }
                    }
                    object_id
                }
                NextObject::Partial(partial) => {
                    let object_id = partial.object.location.object_id;
                    tracing::debug!(object_id, "egress forwarding an arriving subgroup object");
                    match forward_partial(sender.as_mut(), &partial).await {
                        Ok(()) => {
                            task.delivery_stats
                                .record_object_sent(partial.payload_len, partial.received_at);
                            object_count += 1;
                        }
                        Err(PartialForwardFailure::Aborted) => {
                            next = NextObject::Aborted;
                            continue;
                        }
                        Err(PartialForwardFailure::SendFailed(error)) => {
                            let reason =
                                send_failure_reason(&error, Some(object_id), "send_object_failed");
                            return (object_count, reason);
                        }
                    }
                    object_id
                }
                NextObject::Aborted => {
                    task.delivery_stats.record_stream_reset();
                    if let Err(error) = sender.reset(DataStreamResetCode::InternalError).await {
                        tracing::warn!(?error, "failed to reset egress stream sender");
                    }
                    return (object_count, "upstream_aborted");
                }
                NextObject::Finished => {
                    if let Err(error) = sender.close().await {
                        tracing::warn!(?error, "failed to close egress stream sender");
                    }
                    return (object_count, "cache_closed");
                }
            };
            next = match task.source.next(object_id.saturating_add(1)).await {
                SubgroupNext::Cache(next) => next,
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

pub(super) fn subscriber_left_reason(error: &anyhow::Error) -> Option<&'static str> {
    match TransportSendError::in_chain(error)? {
        TransportSendError::Stopped { .. } => Some("peer_stopped"),
        TransportSendError::ConnectionLost { .. } | TransportSendError::SessionError { .. } => {
            Some("connection_lost")
        }
        _ => None,
    }
}

fn send_failure_reason(
    error: &anyhow::Error,
    object_id: Option<u64>,
    fault_reason: &'static str,
) -> &'static str {
    match subscriber_left_reason(error) {
        Some(reason) => {
            tracing::debug!(?object_id, %error, "subscriber left during egress");
            reason
        }
        None => {
            tracing::error!(?object_id, %error, fault_reason, "failed to send on an egress stream");
            fault_reason
        }
    }
}

enum PartialForwardFailure {
    /// The upstream stream ended inside the payload.
    Aborted,
    SendFailed(anyhow::Error),
}

/// Sends `partial` up to its payload, then each payload piece as it arrives.
async fn forward_partial(
    sender: &mut dyn DataSender,
    partial: &PartialObject,
) -> Result<(), PartialForwardFailure> {
    sender
        .send_object(DataObject::SubgroupObjectStart {
            object: partial.object.clone(),
            payload_len: partial.payload_len,
        })
        .await
        .map_err(PartialForwardFailure::SendFailed)?;
    let mut progress_receiver = partial.subscribe();
    let (mut sent_pieces, mut sent_bytes) = (0, 0);
    loop {
        // The writer sets `end` after its last piece, so one borrow sees
        // every piece of an ended object.
        let (pieces, end) = {
            let progress = progress_receiver
                .wait_for(|progress| progress.pieces.len() > sent_pieces || progress.end.is_some())
                .await
                .map_err(|_| PartialForwardFailure::Aborted)?;
            (progress.pieces[sent_pieces..].to_vec(), progress.end)
        };
        for data in pieces {
            sent_pieces += 1;
            sent_bytes += data.len();
            let last = sent_bytes == partial.payload_len;
            sender
                .send_object(DataObject::SubgroupPayload { data, last })
                .await
                .map_err(PartialForwardFailure::SendFailed)?;
        }
        match end {
            None => {}
            Some(PartialEnd::Complete) => return Ok(()),
            Some(PartialEnd::Aborted) => return Err(PartialForwardFailure::Aborted),
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Context;
    use moqt::TransportSendError;

    use super::send_failure_reason;

    fn send_error(error: TransportSendError) -> anyhow::Error {
        Err::<(), _>(error)
            .context("send subgroup object")
            .unwrap_err()
    }

    #[test]
    fn subscriber_leaving_ends_the_stream_as_routine() {
        // Arrange
        let stopped = send_error(TransportSendError::Stopped { code: 0 });
        let lost = send_error(TransportSendError::ConnectionLost {
            reason: "timed out".into(),
        });

        // Act / Assert
        assert_eq!(
            send_failure_reason(&stopped, Some(3), "send_object_failed"),
            "peer_stopped"
        );
        assert_eq!(
            send_failure_reason(&lost, Some(3), "send_object_failed"),
            "connection_lost"
        );
    }

    #[test]
    fn other_send_failures_keep_the_fault_reason() {
        // Arrange
        let error = anyhow::anyhow!("Header not set for StreamSender");

        // Act
        let reason = send_failure_reason(&error, None, "send_header_failed");

        // Assert
        assert_eq!(reason, "send_header_failed");
    }
}
