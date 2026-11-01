use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use moqt::{fetch::JoinPoint, object::ContentExists};
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

/// The relay's state for one track: every publisher feeding it and every
/// subscriber receiving it (draft-14 §8.2). Session ids grow with time, so
/// the last producer is the newest publisher's.
#[derive(Default)]
pub(crate) struct Track {
    pub(crate) producers: BTreeMap<SessionId, Producer>,
    pub(crate) consumers: HashMap<(SessionId, u64), Consumer>,
}

impl Track {
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

    /// A relay's upstream subscription only serves this relay's clients, so a
    /// relay publisher is wanted while a client watches the track and a
    /// client publisher while anyone does.
    pub(crate) fn wants(&self, publisher_peer: SessionPeer) -> bool {
        match publisher_peer {
            SessionPeer::Client => !self.consumers.is_empty(),
            SessionPeer::Relay => self.has_client_consumer(),
        }
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
