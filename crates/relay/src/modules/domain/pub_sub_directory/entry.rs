use std::collections::BTreeMap;

use moqt::{ContentExists, wire::publish_done_status_code};

use crate::modules::domain::{
    session_id::SessionId, session_peer::SessionPeer, track_key::TrackKey,
};

#[derive(Clone, Debug)]
pub(crate) struct ActiveUpstreamSubscription {
    pub(crate) upstream_request_id: u64,
    pub(crate) expires: Option<u64>,
    pub(crate) content_exists: ContentExists,
    pub(crate) origin: UpstreamSubscriptionOrigin,
    pub(crate) publisher_peer: SessionPeer,
}

/// draft-14 §8.2. Session ids grow with time, so the last subscription is the
/// newest publisher's.
#[derive(Clone, Debug, Default)]
pub(crate) struct UpstreamTrack {
    pub(crate) subscriptions: BTreeMap<SessionId, ActiveUpstreamSubscription>,
    pub(crate) downstream_subscriber_count: usize,
    pub(crate) client_downstream_subscriber_count: usize,
}

impl UpstreamTrack {
    pub(crate) fn content_exists(&self) -> ContentExists {
        self.subscriptions
            .values()
            .filter_map(|subscription| match subscription.content_exists {
                ContentExists::True { location } => Some(location),
                ContentExists::False => None,
            })
            .max()
            .map_or(ContentExists::False, |location| ContentExists::True {
                location,
            })
    }

    /// A relay's upstream subscription only serves this relay's clients, so a
    /// relay publisher is wanted while a client watches the track and a
    /// client publisher while anyone does.
    pub(crate) fn wants(&self, publisher_peer: SessionPeer) -> bool {
        match publisher_peer {
            SessionPeer::Client => self.downstream_subscriber_count > 0,
            SessionPeer::Relay => self.client_downstream_subscriber_count > 0,
        }
    }

    pub(crate) fn expires(&self) -> Option<u64> {
        self.subscriptions
            .values()
            .next_back()
            .and_then(|subscription| subscription.expires)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PublishDoneReason {
    pub(crate) status_code: u64,
    pub(crate) error_reason: String,
}

impl PublishDoneReason {
    pub(crate) fn publisher_session_closed() -> Self {
        Self {
            status_code: publish_done_status_code::TRACK_ENDED,
            error_reason: "publisher session closed".to_string(),
        }
    }

    pub(crate) fn malformed_track() -> Self {
        Self {
            status_code: publish_done_status_code::MALFORMED_TRACK,
            error_reason: "malformed track".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpstreamSubscriptionOrigin {
    Publish,
    Subscribe,
}

#[derive(Clone, Debug)]
pub(crate) struct DownstreamSubscription {
    pub(crate) track_key: TrackKey,
    /// The subscription's start location: the Largest Object Location at subscribe time.
    pub(crate) start_location: Option<moqt::Location>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReleasedUpstreamSubscription {
    pub(crate) publisher_session_id: SessionId,
    pub(crate) upstream_request_id: u64,
}

/// `released_upstream_subscriptions` lists the SUBSCRIBE-initiated upstream
/// subscriptions the removal left without the downstream subscriber they need.
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
