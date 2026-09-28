use crate::modules::{
    enums::ContentExists,
    types::{SessionId, TrackKey},
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct UpstreamSubscriptionKey {
    pub(crate) publisher_session_id: SessionId,
    pub(crate) track_namespace: String,
    pub(crate) track_name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveUpstreamSubscription {
    pub(crate) upstream_request_id: u64,
    pub(crate) track_key: TrackKey,
    pub(crate) expires: Option<u64>,
    pub(crate) content_exists: ContentExists,
    pub(crate) downstream_subscriber_count: usize,
    pub(crate) origin: UpstreamSubscriptionOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpstreamSubscriptionOrigin {
    Publish,
    Subscribe,
}

/// Whether a session belongs to an end client or another relay.
/// Client subscriptions own the Redis route for their prefix, so the
/// directory tracks the kind to detect when the last client leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PeerKind {
    Client,
    Relay,
}

#[derive(Clone, Debug)]
pub(crate) struct DownstreamSubscription {
    pub(crate) upstream_key: UpstreamSubscriptionKey,
    /// The subscription's start location: the Largest Object Location at subscribe time.
    pub(crate) start_location: Option<moqt::Location>,
}

#[derive(Clone, Debug)]
pub(crate) struct RemovedDownstreamSubscription {
    pub(crate) upstream_key: UpstreamSubscriptionKey,
    pub(crate) upstream_request_id: u64,
    pub(crate) track_key: TrackKey,
    pub(crate) remaining_downstream_subscriber_count: usize,
    pub(crate) upstream_origin: UpstreamSubscriptionOrigin,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RemovedSessionSubscriptions {
    pub(crate) downstream_subscriptions: Vec<RemovedDownstreamSubscription>,
    pub(crate) upstream_track_keys: Vec<TrackKey>,
    pub(crate) subscribe_namespace_prefixes: Vec<String>,
    pub(crate) publish_namespace_track_namespaces: Vec<String>,
}
