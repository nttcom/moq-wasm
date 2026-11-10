use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use moqt::{
    RequestErrorCode, TrackProperties,
    namespace::NamespaceInterest,
    object::{ContentExists, Location},
    subscribe::FilterType,
};
use tokio::sync::{oneshot, watch};

use crate::modules::{
    data_plane::ingress::ingest_task::IngestTask,
    domain::{
        delivery_control::DeliveryControl,
        delivery_stats::{DeliveryCounters, DeliveryStats},
        new_group_request_gate::NewGroupRequestGate,
        publish_done_reason::PublishDoneReason,
        runner_signals::DownstreamRunnerSignals,
        session_id::SessionId,
        session_peer::SessionPeer,
        track_key::TrackKey,
    },
    session::{session_event::SessionEvent, subscription::UpstreamSubscription},
};

/// One session's subscription to a namespace prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NamespaceSubscription {
    pub(crate) interest: NamespaceInterest,
    pub(crate) peer: SessionPeer,
    /// The Forward State of the PUBLISH messages it receives.
    pub(crate) forward: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProducerOrigin {
    Publish,
    Subscribe,
}

/// What the relay knows about one publisher's subscription to a track.
#[derive(Clone, Debug)]
pub(crate) struct ProducerState {
    pub(crate) subscription: UpstreamSubscription,
    pub(crate) expires: Option<u64>,
    pub(crate) content_exists: ContentExists,
    pub(crate) origin: ProducerOrigin,
    pub(crate) peer: SessionPeer,
    /// The Forward State the relay last asked the publisher for, in its
    /// SUBSCRIBE, PUBLISH_OK or REQUEST_UPDATE.
    pub(crate) forward: bool,
    /// The publisher sent PUBLISH in Forward State 0: it sends the track
    /// only while the relay asks for it (draft-18 §10.10).
    pub(crate) on_demand: bool,
}

/// One publisher feeding a track: its upstream subscription and the ingest
/// reading it into the track's cache. Dropping the producer stops the
/// ingest; the UNSUBSCRIBE, when one is due, is sent by whoever removed it.
pub(crate) struct Producer {
    pub(crate) state: ProducerState,
    ingest: IngestTask,
}

impl Producer {
    pub(crate) fn new(state: ProducerState, ingest: IngestTask) -> Self {
        Self { state, ingest }
    }
}

/// One subscriber receiving a track: its downstream subscription and the
/// signals its egress runner lives by. Dropping the consumer stops the
/// runner without PUBLISH_DONE (the subscriber ended the subscription);
/// `end` stops it with the reason it sends as PUBLISH_DONE.
pub(crate) struct Consumer {
    pub(crate) subscription: DownstreamSubscription,
    pub(crate) peer: SessionPeer,
    runner_stop_sender: oneshot::Sender<PublishDoneReason>,
    delivery_control_sender: watch::Sender<DeliveryControl>,
    delivery_stats: Arc<DeliveryStats>,
}

impl Consumer {
    pub(crate) fn new(
        subscription: DownstreamSubscription,
        peer: SessionPeer,
        delivery_control: DeliveryControl,
    ) -> (Self, DownstreamRunnerSignals) {
        let (runner_stop_sender, stop_receiver) = oneshot::channel();
        let (delivery_control_sender, delivery_control_receiver) = watch::channel(delivery_control);
        let delivery_stats = Arc::new(DeliveryStats::default());
        (
            Self {
                subscription,
                peer,
                runner_stop_sender,
                delivery_control_sender,
                delivery_stats: delivery_stats.clone(),
            },
            DownstreamRunnerSignals {
                stop_receiver,
                delivery_control_receiver,
                delivery_stats,
            },
        )
    }

    pub(crate) fn end(self, reason: PublishDoneReason) {
        let _ = self.runner_stop_sender.send(reason);
    }

    /// See `DeliveryControl::apply_update`. `told_largest` is the Largest
    /// Location the update's answer gave the subscriber, if it gave one; an
    /// update that turns Forward on makes it the Joining Location.
    pub(crate) fn update_delivery(
        &mut self,
        requested: DeliveryControl,
        told_largest: Option<Option<Location>>,
    ) {
        let mut forward_turned_on = false;
        self.delivery_control_sender.send_if_modified(|control| {
            let previous = *control;
            control.apply_update(requested);
            forward_turned_on = !previous.forward && control.forward;
            *control != previous
        });
        if forward_turned_on && let Some(largest_location) = told_largest {
            self.subscription.joining_location = largest_location;
        }
    }

    pub(crate) fn delivery_counters(&self) -> DeliveryCounters {
        self.delivery_stats.counters()
    }

    pub(crate) fn delivery_control(&self) -> DeliveryControl {
        *self.delivery_control_sender.borrow()
    }
}

/// Why a track's creation failed: the SUBSCRIBE_ERROR its subscribers get.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CreationFailure {
    pub(crate) code: RequestErrorCode,
    pub(crate) reason: String,
}

/// A track is `Creating` from the first SUBSCRIBE for it until a producer
/// feeds it (`Created`) or every upstream SUBSCRIBE sent for it failed
/// (`Failed`, after which the track entry is gone).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CreationState {
    Creating,
    Created,
    Failed(CreationFailure),
}

/// Subscribers between finding the track and registering their consumer,
/// by peer: the track wants its producers for them already.
#[derive(Default)]
struct Reservations {
    client: usize,
    relay: usize,
}

/// The relay's state for one track: every publisher feeding it and every
/// subscriber receiving it (draft-14 §8.2). Session ids grow with time, so
/// the last producer is the newest publisher's.
pub(crate) struct Track {
    pub(crate) producers: BTreeMap<SessionId, Producer>,
    pub(crate) consumers: HashMap<(SessionId, u64), Consumer>,
    creation: watch::Sender<CreationState>,
    /// Upstream SUBSCRIBEs sent for the track and not yet answered.
    pending_producers: usize,
    last_failure: Option<CreationFailure>,
    reservations: Reservations,
    /// Sessions a forwarded PUBLISH is on its way to.
    pending_consumers: HashSet<SessionId>,
    pub(crate) new_group_requests: NewGroupRequestGate,
}

impl Track {
    pub(crate) fn new(state: CreationState) -> Self {
        Self {
            producers: BTreeMap::new(),
            consumers: HashMap::new(),
            creation: watch::channel(state).0,
            pending_producers: 0,
            last_failure: None,
            reservations: Reservations::default(),
            pending_consumers: HashSet::new(),
            new_group_requests: NewGroupRequestGate::default(),
        }
    }

    pub(crate) fn is_creating(&self) -> bool {
        *self.creation.borrow() == CreationState::Creating
    }

    pub(crate) fn creation(&self) -> watch::Receiver<CreationState> {
        self.creation.subscribe()
    }

    pub(crate) fn mark_created(&self) {
        self.creation.send_if_modified(|state| {
            let was_creating = *state == CreationState::Creating;
            if was_creating {
                *state = CreationState::Created;
            }
            was_creating
        });
    }

    pub(crate) fn add_pending_producers(&mut self, count: usize) {
        self.pending_producers += count;
    }

    /// One upstream SUBSCRIBE ended without a producer. Returns true, failing
    /// the creation with the last failure, when it was the last one and no
    /// producer came; the caller then removes the entry.
    pub(crate) fn pending_producer_ended(&mut self, failure: Option<CreationFailure>) -> bool {
        self.pending_producers = self.pending_producers.saturating_sub(1);
        if failure.is_some() {
            self.last_failure = failure;
        }
        if self.pending_producers > 0 || !self.producers.is_empty() || !self.is_creating() {
            return false;
        }
        let failure = self.last_failure.take().unwrap_or(CreationFailure {
            code: RequestErrorCode::TrackDoesNotExist,
            reason: "Designated namespace and track name do not exist.".to_string(),
        });
        let _ = self.creation.send(CreationState::Failed(failure));
        true
    }

    pub(crate) fn reserve(&mut self, peer: SessionPeer) {
        match peer {
            SessionPeer::Client => self.reservations.client += 1,
            SessionPeer::Relay => self.reservations.relay += 1,
        }
    }

    pub(crate) fn release_reservation(&mut self, peer: SessionPeer) {
        let count = match peer {
            SessionPeer::Client => &mut self.reservations.client,
            SessionPeer::Relay => &mut self.reservations.relay,
        };
        *count = count.saturating_sub(1);
    }

    pub(crate) fn state(&self) -> TrackState {
        TrackState {
            producers: self
                .producers
                .iter()
                .map(|(session_id, producer)| (*session_id, producer.state.clone()))
                .collect(),
        }
    }

    pub(crate) fn has_client_consumer(&self) -> bool {
        self.consumers
            .values()
            .any(|consumer| consumer.peer == SessionPeer::Client)
    }

    /// Someone receives the track or is about to register for it.
    pub(crate) fn downstream_remains(&self) -> bool {
        !self.consumers.is_empty() || self.reservations.client + self.reservations.relay > 0
    }

    /// A client receives the track or is about to register for it.
    pub(crate) fn client_remains(&self) -> bool {
        self.has_client_consumer() || self.reservations.client > 0
    }

    /// A relay's upstream subscription only serves this relay's clients, so a
    /// relay publisher is wanted while a client watches the track and a
    /// client publisher while anyone does.
    pub(crate) fn wants(&self, publisher_peer: SessionPeer) -> bool {
        match publisher_peer {
            SessionPeer::Client => self.downstream_remains(),
            SessionPeer::Relay => self.client_remains(),
        }
    }

    pub(crate) fn takes(
        &self,
        publisher_session_id: SessionId,
        publisher_peer: SessionPeer,
    ) -> bool {
        !self.producers.contains_key(&publisher_session_id)
            && (self.is_creating() || self.wants(publisher_peer))
    }

    /// The Forward State the upstream subscription of `producer` belongs
    /// in. An on-demand one is in 1 only while a subscriber its publisher
    /// serves (see `wants`) is in Forward State 1 (draft-18 §9.5); any other
    /// stays in 1.
    pub(crate) fn upstream_forward(&self, producer: &ProducerState) -> bool {
        !producer.on_demand
            || self.consumers.values().any(|consumer| {
                consumer.delivery_control().forward
                    && (producer.peer == SessionPeer::Client
                        || consumer.peer == SessionPeer::Client)
            })
    }

    /// Reports the track to the worker of every publisher whose upstream
    /// subscription is not in the Forward State it belongs in. The worker
    /// sends the update (`take_upstream_forward_update`), so the updates to
    /// one publisher go out in the order they were decided.
    pub(crate) fn report_upstream_forward(&self, track_key: &TrackKey) {
        for producer in self.producers.values() {
            if producer.state.forward != self.upstream_forward(&producer.state) {
                producer
                    .ingest
                    .notify_publisher(SessionEvent::UpstreamForwardChanged(track_key.clone()));
            }
        }
    }

    /// The upstream subscription of `publisher_session_id` and the Forward
    /// State to update it to, which is recorded as asked for. `None` when it
    /// is in the Forward State it belongs in.
    pub(crate) fn take_upstream_forward_update(
        &mut self,
        publisher_session_id: SessionId,
    ) -> Option<(UpstreamSubscription, bool)> {
        let forward = self.upstream_forward(&self.producers.get(&publisher_session_id)?.state);
        let state = &mut self.producers.get_mut(&publisher_session_id)?.state;
        if state.forward == forward {
            return None;
        }
        state.forward = forward;
        Some((state.subscription.clone(), forward))
    }

    pub(crate) fn begin_forwarded_publish(&mut self, session_id: SessionId) -> bool {
        !self
            .consumers
            .keys()
            .any(|(consumer_session_id, _)| *consumer_session_id == session_id)
            && self.pending_consumers.insert(session_id)
    }

    pub(crate) fn end_forwarded_publish(&mut self, session_id: SessionId) {
        self.pending_consumers.remove(&session_id);
    }
}

/// A track as a sequence reads it: its producers without their ingests.
#[derive(Clone, Debug, Default)]
pub(crate) struct TrackState {
    pub(crate) producers: BTreeMap<SessionId, ProducerState>,
}

impl TrackState {
    pub(crate) fn content_exists(&self) -> ContentExists {
        self.largest_location()
            .map_or(ContentExists::False, |location| ContentExists::True {
                location,
            })
    }

    /// The largest Largest Location of its producers.
    pub(crate) fn largest_location(&self) -> Option<Location> {
        self.producers
            .values()
            .filter_map(|producer| match producer.content_exists {
                ContentExists::True { location } => Some(location),
                ContentExists::False => None,
            })
            .max()
    }

    pub(crate) fn expires(&self) -> Option<u64> {
        self.producers
            .values()
            .next_back()
            .and_then(|producer| producer.expires)
    }

    /// The upstream subscriptions of the publishers whose track has dynamic
    /// groups (draft-18 §12.6): the ones a NEW_GROUP_REQUEST may go to.
    pub(crate) fn dynamic_group_subscriptions(&self) -> Vec<(SessionId, UpstreamSubscription)> {
        self.producers
            .iter()
            .filter(|(_, producer)| {
                producer
                    .subscription
                    .as_moqt()
                    .track_properties()
                    .dynamic_groups()
            })
            .map(|(session_id, producer)| (*session_id, producer.subscription.clone()))
            .collect()
    }

    /// The newest publisher's track properties, which the relay passes
    /// downstream unchanged.
    pub(crate) fn track_properties(&self) -> TrackProperties {
        self.producers
            .values()
            .next_back()
            .map(|producer| producer.subscription.as_moqt().track_properties().clone())
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MatchingPublication {
    Namespace(String),
    Track(TrackKey),
}

/// What a Joining FETCH of the subscription resolves against (draft-14
/// §9.16.2, draft-18 §10.12.2).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DownstreamSubscription {
    pub(crate) track_key: TrackKey,
    pub(crate) filter_type: FilterType,
    /// The Largest Location the subscription was accepted with, `None` when
    /// the track had no objects; draft-18 §5.1 calls it the Joining Location.
    pub(crate) joining_location: Option<Location>,
}

#[derive(Clone, Debug)]
pub(crate) struct ReleasedUpstreamSubscription {
    pub(crate) publisher_session_id: SessionId,
    pub(crate) subscription: UpstreamSubscription,
}

/// `released_upstream_subscriptions` lists the upstream subscriptions the
/// removal left without the downstream subscriber they need.
#[derive(Clone, Debug)]
pub(crate) struct RemovedDownstreamSubscription {
    pub(crate) track_key: TrackKey,
    pub(crate) released_upstream_subscriptions: Vec<ReleasedUpstreamSubscription>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RemovedSessionSubscriptions {
    pub(crate) downstream_subscriptions: Vec<RemovedDownstreamSubscription>,
    pub(crate) upstream_track_keys: Vec<TrackKey>,
    pub(crate) subscribe_namespace_prefixes: Vec<String>,
    pub(crate) publish_namespace_track_namespaces: Vec<String>,
}

#[cfg(test)]
mod tests {
    use moqt::object::{KeyValuePair, VariantType};

    use super::*;
    use crate::modules::test_support::{
        directory_fixtures::upstream_carrying,
        relay_harness::fixtures::subscription::unknown_track_properties,
    };

    #[test]
    fn the_newest_publishers_track_properties_replace_the_others_unmerged() {
        // Arrange: the older set holds a property the newer one lacks
        let mut older_pairs: Vec<KeyValuePair> =
            unknown_track_properties(1).iter().cloned().collect();
        older_pairs.push(KeyValuePair {
            key: 0x3A,
            value: VariantType::Even(7),
        });
        let mut older = moqt::TrackProperties::from_pairs(older_pairs);
        older.set_max_cache_duration(Some(1_000));
        let newer = moqt::TrackProperties::from_pairs(vec![KeyValuePair {
            key: 0x38,
            value: VariantType::Even(2),
        }]);
        let mut upstream_track = TrackState::default();
        upstream_track.producers.insert(1, upstream_carrying(older));
        upstream_track
            .producers
            .insert(3, upstream_carrying(newer.clone()));

        // Act
        let track_properties = upstream_track.track_properties();

        // Assert: draft-18 §2.5 forbids merging sets
        assert_eq!(track_properties, newer);
    }
}
