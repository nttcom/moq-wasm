use moqt::{
    TransportSendError,
    object::{DataStreamResetCode, DatagramDelivery, ForwardingPreference},
};
use std::{sync::Arc, time::Duration};

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
        delivery_control::DeliveryControl,
        delivery_stats::DeliveryStats,
        single_preference::{SinglePreference, converted_datagram_subgroup},
        track_key::TrackKey,
    },
    session::{
        data_object::DataObject,
        data_sender::{
            DataSender, datagram_sender::DatagramSender, stream_sender_factory::StreamSenderFactory,
        },
    },
};

use super::{group_sequence::GroupSequence, runner::Downstream, scheduler::GroupSendTask};

/// Per subscription. Only a subscriber that stopped reading reaches it,
/// since a live track keeps a few subgroups open; the scheduler then blocks
/// on the task channel, and its lag scan schedules the missed groups that are
/// still cached.
const MAX_CONCURRENT_SEND_TASKS: usize = 256;
const STREAM_CREDIT_TIMEOUT: Duration = Duration::from_secs(10);

type SharedStreamSenderFactory = Arc<Mutex<Box<dyn StreamSenderFactory>>>;

pub(crate) struct GroupSender {
    track_key: TrackKey,
    cache: Arc<TrackCache>,
    downstream: Downstream,
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
    /// The group lies before the Start Location, or Forward is 0 or has been
    /// 0 since the task last delivered. An open stream is reset (draft-18
    /// §11.4.3), and the subgroup may continue on a new stream (see
    /// `SubgroupSource::held`).
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

    /// Waits until an update releases the subgroup and returns the object id
    /// to continue at: `unsent`, or past the Largest Object at that update if
    /// later (`DeliveryControl::resume_object_id`). `None` when nothing is
    /// left to send: the subgroup ended, its group lies past the End Group or
    /// at or before that Largest Object, the registration is gone, or the
    /// scheduler has returned (no update can release it then, and the runner
    /// sends PUBLISH_DONE only after every task ends).
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
        downstream: Downstream,
        receiver: mpsc::Receiver<GroupSendTask>,
        delivery_stats: Arc<DeliveryStats>,
        delivery_control_receiver: watch::Receiver<DeliveryControl>,
    ) -> Self {
        Self {
            track_key,
            cache,
            downstream,
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

    fn stream_factory(
        &self,
        stream_factory: &mut Option<SharedStreamSenderFactory>,
    ) -> SharedStreamSenderFactory {
        stream_factory
            .get_or_insert_with(|| {
                Arc::new(Mutex::new(
                    self.downstream
                        .publisher
                        .new_stream_factory(&self.downstream.subscription),
                ))
            })
            .clone()
    }

    pub(crate) async fn run(mut self) {
        let mut stream_factory: Option<SharedStreamSenderFactory> = None;
        let mut joinset = JoinSet::<()>::new();
        let track_alias = self.downstream.subscription.track_alias();
        let subscriber_priority = self.downstream.subscription.subscriber_priority();
        let group_order = self.downstream.subscription.group_order();
        // draft-14 §10 gives a track one forwarding preference; a draft-18
        // upstream may mix them (§11).
        let keeps_one_preference = !self.downstream.publisher.mixes_forwarding_preferences();
        let mut single_preference: Option<SinglePreference> = None;

        loop {
            tokio::select! {
                task = self.receiver.recv(),
                    if !*self.schedule_over_sender.borrow() && joinset.len() < MAX_CONCURRENT_SEND_TASKS => {
                    let Some(task) = task else {
                        self.schedule_over_sender.send_replace(true);
                        continue;
                    };
                    let arrived_as_datagram = matches!(task.key, SubgroupKey::Datagram { .. });
                    let as_datagram = if keeps_one_preference {
                        let preference = *single_preference.get_or_insert_with(|| {
                            let first = if arrived_as_datagram {
                                SinglePreference::Datagrams
                            } else {
                                SinglePreference::Subgroups
                            };
                            self.cache
                                .fix_single_preference(self.downstream.session_id, first)
                        });
                        preference == SinglePreference::Datagrams
                    } else {
                        arrived_as_datagram
                    };
                    match (task.key, as_datagram) {
                        (SubgroupKey::Stream { group_id, subgroup_id }, false) => {
                            let factory = self.stream_factory(&mut stream_factory);
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
                        (SubgroupKey::Datagram { group_id }, false) => {
                            let factory = self.stream_factory(&mut stream_factory);
                            let group_sequence = self.group_sequence.sequence_of(group_id);
                            joinset.spawn(Self::send_datagrams_on_streams(StreamSendTask {
                                source: self.source(&task),
                                object_id: task.object_id,
                                factory,
                                delivery_stats: self.delivery_stats.clone(),
                                subscriber_priority,
                                group_order,
                                group_sequence,
                            }));
                        }
                        (_, true) => {
                            let sender = self
                                .downstream
                                .publisher
                                .new_datagram(&self.downstream.subscription);
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
        let sent = Self::send_stream(task).await;
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
                // The open waits for the subscriber's stream credit, so only the
                // priority is kept across it and the object is read again once the
                // stream is open: holding the object would keep it alive past its
                // cache expiry. Every object of a subgroup has this priority
                // (draft-14 §10.4.2).
                drop(next);
                match Self::open_stream(&task, group_id, subgroup_id, publisher_priority).await {
                    Ok(opened) => sender = Some(opened),
                    Err(end_reason) => {
                        sent.end_reason = end_reason;
                        return sent;
                    }
                }
                continue;
            }
            let Some(open) = sender.as_mut() else {
                unreachable!("the stream was opened above");
            };
            match next {
                NextObject::Object(object) => match object.load().await {
                    Ok(loaded) => {
                        tracing::debug!(object_id, "egress sending subgroup object");
                        if let Err(error) =
                            open.send_object(DataObject::SubgroupObject(loaded)).await
                        {
                            sent.end_reason =
                                send_failure_reason(&error, Some(object_id), "send_object_failed");
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
                        Err(PartialForwardFailure::SendFailed(error)) => {
                            sent.end_reason =
                                send_failure_reason(&error, Some(object_id), "send_object_failed");
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
    /// object, which carries `publisher_priority`. Fails with the end reason
    /// of the subgroup.
    async fn open_stream(
        task: &StreamSendTask,
        group_id: u64,
        subgroup_id: u64,
        publisher_priority: u8,
    ) -> Result<Box<dyn DataSender>, &'static str> {
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
        let opened = tokio::time::timeout(STREAM_CREDIT_TIMEOUT, async {
            let mut factory = task.factory.lock().await;
            factory.next(priority).await
        })
        .await;
        let mut sender = match opened {
            Ok(Ok(sender)) => sender,
            Ok(Err(e)) => {
                tracing::error!(error = ?e, "failed to open stream sender");
                return Err("open_failed");
            }
            Err(_elapsed) => {
                tracing::debug!("no stream credit from the subscriber; giving up on the group");
                return Err("no_stream_credit");
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
            return Err(send_failure_reason(&error, None, "send_header_failed"));
        }
        Ok(sender)
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

    /// Sends each object of a datagram group alone on a subgroup stream in
    /// the range reserved for converted datagrams, apart from every real
    /// subgroup of the group (see `CONVERTED_DATAGRAM_SUBGROUPS`). An Object
    /// ID past the range is dropped.
    async fn send_datagrams_on_streams(mut task: StreamSendTask) {
        let group_id = task.source.key.group_id();
        let mut cursor = task.object_id;
        loop {
            let object = match task.source.next(cursor).await {
                SubgroupNext::Cache(NextObject::Object(object)) => object,
                // Each object's stream is closed once it is sent, so a hold
                // has no stream to reset.
                SubgroupNext::Held => match task.source.held(cursor).await {
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
            let Some(subgroup_id) = converted_datagram_subgroup(object_id) else {
                tracing::debug!(
                    object_id,
                    "dropping a datagram past the converted subgroups"
                );
                continue;
            };
            let mut loaded = match object.load().await {
                Ok(loaded) => loaded,
                Err(error) => {
                    tracing::debug!(object_id, ?error, "skipping unloadable object");
                    continue;
                }
            };
            loaded.forwarding = ForwardingPreference::Subgroup { subgroup_id };
            let priority = moqt::object::StreamPriority {
                subscriber_priority: task.subscriber_priority,
                publisher_priority: loaded.publisher_priority,
                group_order: task.group_order,
                group_sequence: task.group_sequence,
                subgroup_id,
            };
            let opened = {
                let mut factory = task.factory.lock().await;
                factory.next(priority).await
            };
            let mut sender = match opened {
                Ok(sender) => sender,
                Err(error) => {
                    tracing::error!(?error, "failed to open stream sender");
                    return;
                }
            };
            task.delivery_stats.record_stream_opened();
            let start = moqt::object::SubgroupStart {
                group_id,
                subgroup_id,
                publisher_priority: loaded.publisher_priority,
                ends_group: loaded.ends_group,
            };
            loaded.ends_group = false;
            tracing::debug!(
                group_id,
                object_id,
                "egress sending datagram object on a stream"
            );
            let sent = async {
                sender
                    .send_object(DataObject::SubgroupHeader(start))
                    .await?;
                sender
                    .send_object(DataObject::SubgroupObject(loaded))
                    .await?;
                sender.close().await
            };
            if let Err(error) = sent.await {
                tracing::error!(
                    ?error,
                    object_id,
                    "failed to send datagram object on a stream"
                );
                return;
            }
            task.delivery_stats
                .record_object_sent(object.payload_len(), object.received_at);
        }
    }

    async fn send_datagram_task(
        track_alias: u64,
        mut source: SubgroupSource,
        object_id: u64,
        mut sender: Box<dyn DatagramSender>,
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
            let mut loaded = match object.load().await {
                Ok(loaded) => loaded,
                Err(error) => {
                    tracing::debug!(object_id, ?error, "skipping unloadable object");
                    continue;
                }
            };
            loaded.forwarding = ForwardingPreference::Datagram;
            tracing::debug!(
                track_alias,
                group_id = source.key.group_id(),
                object_id,
                "egress sending datagram object"
            );
            match sender.send(loaded).await {
                Ok(DatagramDelivery::TooLarge) => delivery_stats.record_datagram_dropped(),
                Ok(_) => {
                    delivery_stats.record_object_sent(object.payload_len(), object.received_at)
                }
                Err(_) => return,
            }
        }
    }
}

/// What a stream task sent, and why it ended.
struct SentSubgroup {
    object_count: u64,
    end_reason: &'static str,
    /// The next object id not sent.
    unsent: u64,
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
    use std::{sync::Arc, time::Duration};

    use anyhow::Context;
    use moqt::TransportSendError;
    use tokio::sync::{mpsc, watch};

    use super::{
        Downstream, GroupSendTask, GroupSender, MAX_CONCURRENT_SEND_TASKS, STREAM_CREDIT_TIMEOUT,
        send_failure_reason,
    };
    use crate::modules::{
        data_plane::cache::track_cache::TrackCache,
        domain::track_key::TrackKey,
        test_support::{
            directory_fixtures::delivery_control,
            mock_session::wait_until,
            relay_harness::{
                MockPublisher, MockPublisherObservers,
                fixtures::{
                    cached_object::{open_group, stream_key},
                    subscription::make_subscription,
                },
            },
        },
    };

    fn run_group_sender(
        cache: &Arc<TrackCache>,
        publisher: MockPublisher,
    ) -> mpsc::Sender<GroupSendTask> {
        let (task_sender, task_receiver) = mpsc::channel(1);
        let group_sender = GroupSender::new(
            TrackKey::new("ns", "track"),
            cache.clone(),
            Downstream {
                session_id: 2,
                publisher: Arc::new(publisher),
                subscription: make_subscription(moqt::subscribe::FilterType::LargestObject),
            },
            task_receiver,
            Arc::default(),
            watch::channel(delivery_control(true)).1,
        );
        tokio::spawn(group_sender.run());
        task_sender
    }

    fn send_task(group_id: u64) -> GroupSendTask {
        GroupSendTask {
            key: stream_key(group_id),
            generation: 0,
            object_id: 0,
            resumes: 0,
        }
    }

    #[tokio::test]
    async fn a_stream_waiting_for_stream_credit_does_not_hold_its_first_object() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        let _open = open_group(&cache, 0, &[0]);
        let (publisher, MockPublisherObservers { mut priorities, .. }) = MockPublisher::channel();
        let task_sender = run_group_sender(&cache, publisher.stalling_stream_opens());

        // Act
        task_sender.send(send_task(0)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), priorities.recv())
            .await
            .expect("the task should start opening its stream")
            .expect("the publisher should stay alive");

        // Assert: only the cache and this lookup hold the object
        let first = cache
            .next_subgroup_object_or_wait(stream_key(0), 0, 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(Arc::strong_count(&first), 2);
    }

    #[tokio::test]
    async fn a_task_without_stream_credit_gives_up_on_its_group() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        let _first = open_group(&cache, 0, &[0]);
        let _second = open_group(&cache, 1, &[0]);
        let (publisher, MockPublisherObservers { mut priorities, .. }) = MockPublisher::channel();
        let task_sender = run_group_sender(&cache, publisher.stalling_stream_opens());
        task_sender.send(send_task(0)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), priorities.recv())
            .await
            .expect("the task should start opening its stream")
            .expect("the publisher should stay alive");
        tokio::time::pause();

        // Act
        tokio::time::advance(STREAM_CREDIT_TIMEOUT).await;
        task_sender.send(send_task(1)).await.unwrap();

        // Assert: group 1 starts opening its stream, so group 0's task released the shared factory
        tokio::time::timeout(Duration::from_secs(3), priorities.recv())
            .await
            .expect("group 1 should start opening its stream")
            .expect("the publisher should stay alive");
    }

    #[tokio::test]
    async fn send_tasks_beyond_the_cap_wait_until_one_ends() {
        // Arrange: every subgroup is open and empty, so each task waits for its first object
        let cache = Arc::new(TrackCache::new());
        let task_count = MAX_CONCURRENT_SEND_TASKS as u64 + 1;
        let mut opens: Vec<_> = (0..task_count)
            .map(|group_id| open_group(&cache, group_id, &[]))
            .collect();
        let (publisher, _observers) = MockPublisher::channel();
        let task_sender = run_group_sender(&cache, publisher);

        // Act
        for group_id in 0..task_count {
            task_sender.send(send_task(group_id)).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Assert: the task past the cap stays in the channel
        assert_eq!(task_sender.capacity(), 0);

        // Act
        opens.remove(0).finish();

        // Assert: the ended task frees a slot for the waiting one
        wait_until(|| task_sender.capacity() == 1)
            .await
            .expect("the group sender should take the waiting task");
    }

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
