use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use moqt::{RequestErrorCode, fetch::JoinPoint, object::ContentExists};
use tokio::sync::{oneshot, watch};

use crate::modules::{
    data_plane::ingress::ingest_task::IngestTask,
    domain::{
        delivery_control::{DeliveryControl, UpdateOutcome},
        delivery_stats::{DeliveryCounters, DeliveryStats},
        publish_done_reason::PublishDoneReason,
        runner_signals::DownstreamRunnerSignals,
        session_id::SessionId,
        session_peer::SessionPeer,
        track_key::TrackKey,
    },
    session::subscription::UpstreamSubscription,
};

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
}

/// One publisher feeding a track: its upstream subscription and the ingest
/// reading it into the track's cache. Dropping the producer stops the
/// ingest; the UNSUBSCRIBE, when one is due, is sent by whoever removed it.
pub(crate) struct Producer {
    pub(crate) state: ProducerState,
    _ingest: IngestTask,
}

impl Producer {
    pub(crate) fn new(state: ProducerState, ingest: IngestTask) -> Self {
        Self {
            state,
            _ingest: ingest,
        }
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

    /// See `DeliveryControl::apply_update`.
    pub(crate) fn update_delivery(&self, update: DeliveryControl) -> UpdateOutcome {
        let mut outcome = UpdateOutcome::Applied;
        self.delivery_control_sender.send_if_modified(|control| {
            let previous = *control;
            outcome = control.apply_update(update);
            *control != previous
        });
        outcome
    }

    pub(crate) fn delivery_counters(&self) -> DeliveryCounters {
        self.delivery_stats.counters()
    }

    #[cfg(test)]
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
}

impl Track {
    fn new(state: CreationState) -> Self {
        Self {
            producers: BTreeMap::new(),
            consumers: HashMap::new(),
            creation: watch::channel(state).0,
            pending_producers: 0,
            last_failure: None,
            reservations: Reservations::default(),
            pending_consumers: HashSet::new(),
        }
    }

    /// A track whose first SUBSCRIBE is being forwarded upstream.
    pub(crate) fn creating() -> Self {
        Self::new(CreationState::Creating)
    }

    /// A track a PUBLISH feeds without any SUBSCRIBE having asked for it.
    pub(crate) fn created() -> Self {
        Self::new(CreationState::Created)
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

    /// One upstream SUBSCRIBE ended without a producer. When it was the last
    /// one and no producer came, the creation fails with the last failure and
    /// the track entry is to be removed: returns true then.
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
        self.producers
            .values()
            .filter_map(|producer| match producer.content_exists {
                ContentExists::True { location } => Some(location),
                ContentExists::False => None,
            })
            .max()
            .map_or(ContentExists::False, |location| ContentExists::True {
                location,
            })
    }

    pub(crate) fn expires(&self) -> Option<u64> {
        self.producers
            .values()
            .next_back()
            .and_then(|producer| producer.expires)
    }

    /// The newest publisher's MAX_CACHE_DURATION in milliseconds, which the
    /// relay passes downstream unchanged.
    pub(crate) fn max_cache_duration(&self) -> Option<u64> {
        self.producers
            .values()
            .next_back()
            .and_then(|producer| producer.subscription.as_moqt().max_cache_duration())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MatchingPublication {
    Namespace(String),
    Track(TrackKey),
}

#[derive(Clone, Debug)]
pub(crate) struct DownstreamSubscription {
    pub(crate) track_key: TrackKey,
    pub(crate) join_point: JoinPoint,
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
