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
    /// Set once the scheduler has scheduled its last task.
    schedule_over_sender: watch::Sender<bool>,
}

/// Where a send task reads its subgroup from: the cache, while the
/// subscription forwards and its range holds the group.
struct SubgroupSource {
    key: SubgroupKey,
    generation: usize,
    cache: Arc<TrackCache>,
    delivery_control_receiver: watch::Receiver<DeliveryControl>,
    /// `DeliveryControl::resumes` the task last delivered under.
    resumes: u64,
    schedule_over_receiver: watch::Receiver<bool>,
}

enum SubgroupNext {
    Cache(NextObject),
    Malformed,
    /// draft-14 §10.4.3: the group lies at or past the End Group, so an open
    /// stream is reset and the subgroup is not delivered any more.
    PastEnd,
    /// The group lies before the Start Location, or Forward is 0 or was
    /// since the task last delivered. An open stream is reset, as no object
    /// may be sent at Forward 0 (draft-14 §5.1, draft-18 §5.1; draft-18
    /// §11.4.3 lists both among the reasons to reset), and the subgroup may
    /// continue on a new stream (see `SubgroupSource::held`).
    Held,
}

impl SubgroupSource {
    async fn next(&mut self, object_id: u64) -> SubgroupNext {
        let group_id = self.key.group_id();
        let resumes = self.resumes;
        let receiver = &mut self.delivery_control_receiver;
        let interrupted = async {
            let interruption = receiver
                .wait_for(|control| {
                    !control.contains_group(group_id)
                        || !control.forward
                        || control.resumes != resumes
                })
                .await
                .map(|control| {
                    if control.is_past_end(group_id) {
                        SubgroupNext::PastEnd
                    } else {
                        SubgroupNext::Held
                    }
                });
            match interruption {
                Ok(interruption) => interruption,
                // Without its registration the range can no longer change.
                Err(_) => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            interruption = interrupted => interruption,
            next = self.cache.next_subgroup_object_or_wait(self.key, self.generation, object_id) => {
                match next {
                    Ok(next) => SubgroupNext::Cache(next),
                    Err(TrackMalformed) => SubgroupNext::Malformed,
                }
            }
        }
    }

    /// Waits, sending nothing, until an update lets the subgroup be
    /// delivered again, and returns the object id to go on at given that
    /// `unsent` is the next one not sent: past the Largest Object at that
    /// update, as for a new subscription (see
    /// `DeliveryControl::resume_object_id`). `None` when nothing is left to
    /// send: the subgroup ended meanwhile, its group lies past the End Group
    /// or at or before that Largest Object, the registration is gone, or the
    /// scheduler is done. A done scheduler restarts nothing any more, and the
    /// runner sends PUBLISH_DONE only once every task has ended.
    async fn held(&mut self, unsent: u64) -> Option<u64> {
        let group_id = self.key.group_id();
        let (key, generation, cache) = (self.key, self.generation, self.cache.clone());
        let subgroup_ended = async move {
            let mut cursor = unsent;
            loop {
                let object_id = match cache
                    .next_subgroup_object_or_wait(key, generation, cursor)
                    .await
                {
                    Ok(NextObject::Object(object)) => object.object.location.object_id,
                    Ok(NextObject::Partial(partial)) => partial.object.location.object_id,
                    _ => break,
                };
                cursor = object_id.saturating_add(1);
            }
        };
        let schedule_over = self.schedule_over_receiver.wait_for(|over| *over);
        let control = {
            let released = self.delivery_control_receiver.wait_for(|control| {
                control.is_past_end(group_id)
                    || (control.forward && control.contains_group(group_id))
            });
            tokio::select! {
                biased;
                released = released => *released.ok()?,
                () = subgroup_ended => return None,
                _ = schedule_over => return None,
            }
        };
        if control.is_past_end(group_id) {
            return None;
        }
        self.resumes = control.resumes;
        control.resume_object_id(group_id, unsent)
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
            schedule_over_sender: watch::channel(false).0,
        }
    }

    fn source(&self, task: &GroupSendTask) -> SubgroupSource {
        SubgroupSource {
            key: task.key,
            generation: task.generation,
            cache: self.cache.clone(),
            delivery_control_receiver: self.delivery_control_receiver.clone(),
            resumes: task.resumes,
            schedule_over_receiver: self.schedule_over_sender.subscribe(),
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
                task = self.receiver.recv(), if !*self.schedule_over_sender.borrow() => {
                    let Some(task) = task else {
                        self.schedule_over_sender.send_replace(true);
                        continue;
                    };
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
        let sent = Self::send_stream(task).await;
        let span = Span::current();
        span.record("object_count", sent.object_count);
        span.record("end_reason", sent.end_reason);
    }

    /// Sends the subgroup on a stream opened at its first object to send. An
    /// update that holds the subgroup (Forward 0, or the start moved past
    /// its group) resets the stream. Once a later update releases it, the
    /// subgroup continues on a new stream past the Largest Object at that
    /// update: draft-18 §11.4.3 lets the publisher reopen a subgroup after
    /// Forward 0→1, and §10.9.1 serves a start moved back like a new
    /// subscription.
    async fn send_stream(mut task: StreamSendTask) -> SentSubgroup {
        let SubgroupKey::Stream {
            group_id,
            subgroup_id,
        } = task.source.key
        else {
            unreachable!("run() spawns send_stream_task only for SubgroupKey::Stream");
        };
        let mut sent = SentSubgroup {
            object_count: 0,
            end_reason: "no_objects",
            unsent: task.object_id,
        };
        let mut sender: Option<Box<dyn DataSender>> = None;
        loop {
            let (next, (object_id, publisher_priority)) = match task.source.next(sent.unsent).await
            {
                SubgroupNext::Cache(NextObject::Object(object)) => {
                    let first = &object.object;
                    let first = (first.location.object_id, first.publisher_priority);
                    (NextObject::Object(object), first)
                }
                SubgroupNext::Cache(NextObject::Partial(partial)) => {
                    let first = &partial.object;
                    let first = (first.location.object_id, first.publisher_priority);
                    (NextObject::Partial(partial), first)
                }
                SubgroupNext::Cache(NextObject::Finished) => {
                    if let Some(mut sender) = sender {
                        if let Err(error) = sender.close().await {
                            tracing::warn!(?error, "failed to close egress stream sender");
                        }
                        sent.end_reason = "cache_closed";
                    }
                    return sent;
                }
                SubgroupNext::Cache(NextObject::Aborted) => {
                    // draft-14 §10.4.3: a subgroup that ended upstream without a FIN
                    // may be missing objects, so the downstream stream is reset.
                    if let Some(sender) = sender {
                        Self::reset(
                            sender,
                            &task.delivery_stats,
                            DataStreamResetCode::InternalError,
                        )
                        .await;
                        sent.end_reason = "upstream_aborted";
                    }
                    return sent;
                }
                // The runner terminates the subscription with PUBLISH_DONE; a FIN
                // here would claim the partial subgroup is complete.
                SubgroupNext::Malformed => {
                    sent.end_reason = "malformed_track";
                    return sent;
                }
                SubgroupNext::PastEnd => {
                    if let Some(sender) = sender {
                        Self::reset(sender, &task.delivery_stats, DataStreamResetCode::Cancelled)
                            .await;
                    }
                    sent.end_reason = "left_range";
                    return sent;
                }
                SubgroupNext::Held => {
                    if let Some(sender) = sender.take() {
                        tracing::debug!("subgroup held by an update; resetting egress stream");
                        Self::reset(sender, &task.delivery_stats, DataStreamResetCode::Cancelled)
                            .await;
                    }
                    match task.source.held(sent.unsent).await {
                        Some(object_id) => sent.unsent = object_id,
                        None => {
                            sent.end_reason = "not_resumed";
                            return sent;
                        }
                    }
                    continue;
                }
            };
            if sender.is_none() {
                match Self::open_stream(&task, group_id, subgroup_id, publisher_priority).await {
                    Some(opened) => sender = Some(opened),
                    None => {
                        sent.end_reason = "open_failed";
                        return sent;
                    }
                }
            }
            let Some(open) = sender.as_mut() else {
                unreachable!("the stream was opened above");
            };
            match next {
                NextObject::Object(object) => match object.load().await {
                    Ok(loaded) => {
                        tracing::debug!(object_id, "egress sending subgroup object");
                        if open
                            .send_object(DataObject::SubgroupObject(loaded))
                            .await
                            .is_err()
                        {
                            tracing::error!(object_id, "failed to send subgroup object");
                            sent.end_reason = "send_object_failed";
                            return sent;
                        }
                        task.delivery_stats
                            .record_object_sent(object.payload_len(), object.received_at);
                        sent.object_count += 1;
                    }
                    // Its payload could not be read: skipped, as an expired object is.
                    Err(error) => {
                        tracing::debug!(object_id, ?error, "skipping unloadable object")
                    }
                },
                NextObject::Partial(partial) => {
                    tracing::debug!(object_id, "egress forwarding an arriving subgroup object");
                    match forward_partial(open.as_mut(), &partial).await {
                        Ok(()) => {
                            task.delivery_stats
                                .record_object_sent(partial.payload_len, partial.received_at);
                            sent.object_count += 1;
                        }
                        // draft-14 §10.4.3: the upstream stream ended inside the
                        // payload, so the downstream stream is reset.
                        Err(PartialForwardFailure::Aborted) => {
                            if let Some(sender) = sender {
                                Self::reset(
                                    sender,
                                    &task.delivery_stats,
                                    DataStreamResetCode::InternalError,
                                )
                                .await;
                            }
                            sent.end_reason = "upstream_aborted";
                            return sent;
                        }
                        Err(PartialForwardFailure::SendFailed) => {
                            tracing::error!(object_id, "failed to send subgroup object");
                            sent.end_reason = "send_object_failed";
                            return sent;
                        }
                    }
                }
                NextObject::Finished | NextObject::Aborted => {
                    unreachable!("only objects reach this point")
                }
            }
            sent.unsent = object_id.saturating_add(1);
        }
    }

    /// Opens the downstream stream and sends the SUBGROUP_HEADER of its first
    /// object, which carries `publisher_priority`.
    async fn open_stream(
        task: &StreamSendTask,
        group_id: u64,
        subgroup_id: u64,
        publisher_priority: u8,
    ) -> Option<Box<dyn DataSender>> {
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
                tracing::error!(?e, "failed to open stream sender");
                return None;
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
            tracing::error!(?error, "failed to send subgroup header");
            return None;
        }
        Some(sender)
    }

    async fn reset(
        mut sender: Box<dyn DataSender>,
        delivery_stats: &DeliveryStats,
        code: DataStreamResetCode,
    ) {
        delivery_stats.record_stream_reset();
        if let Err(error) = sender.reset(code).await {
            tracing::warn!(?error, "failed to reset egress stream sender");
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
        loop {
            let object = match source.next(cursor).await {
                SubgroupNext::Cache(NextObject::Object(object)) => object,
                SubgroupNext::Held => match source.held(cursor).await {
                    Some(object_id) => {
                        cursor = object_id;
                        continue;
                    }
                    None => return,
                },
                _ => return,
            };
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

/// How a stream task ended.
struct SentSubgroup {
    object_count: u64,
    end_reason: &'static str,
    /// The next object id not sent.
    unsent: u64,
}

enum PartialForwardFailure {
    /// The upstream stream ended inside the payload.
    Aborted,
    SendFailed,
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
        .map_err(|_| PartialForwardFailure::SendFailed)?;
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
                .map_err(|_| PartialForwardFailure::SendFailed)?;
        }
        match end {
            None => {}
            Some(PartialEnd::Complete) => return Ok(()),
            Some(PartialEnd::Aborted) => return Err(PartialForwardFailure::Aborted),
        }
    }
}
