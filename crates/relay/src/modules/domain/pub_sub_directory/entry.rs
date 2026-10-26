use std::collections::BTreeMap;

use moqt::{
    TrackProperties,
    namespace::NamespaceInterest,
    object::{ContentExists, Location},
    publish::PublishDoneCode,
    subscribe::FilterType,
};

use crate::modules::{
    domain::{session_id::SessionId, session_peer::SessionPeer, track_key::TrackKey},
    session::subscription::UpstreamSubscription,
};

/// One session's subscription to a namespace prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NamespaceSubscription {
    pub(crate) interest: NamespaceInterest,
    pub(crate) peer: SessionPeer,
    /// The Forward State of the PUBLISH messages it receives.
    pub(crate) forward: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveUpstreamSubscription {
    pub(crate) subscription: UpstreamSubscription,
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

    /// The newest publisher's track properties, which the relay passes
    /// downstream unchanged.
    pub(crate) fn track_properties(&self) -> TrackProperties {
        self.subscriptions
            .values()
            .next_back()
            .map(|subscription| {
                subscription
                    .subscription
                    .as_moqt()
                    .track_properties()
                    .clone()
            })
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PublishDoneReason {
    pub(crate) status_code: PublishDoneCode,
    pub(crate) error_reason: String,
}

impl PublishDoneReason {
    pub(crate) fn publisher_session_closed() -> Self {
        Self {
            status_code: PublishDoneCode::TrackEnded,
            error_reason: "publisher session closed".to_string(),
        }
    }

    pub(crate) fn subscription_ended() -> Self {
        Self {
            status_code: PublishDoneCode::SubscriptionEnded,
            error_reason: "end of subscription range reached".to_string(),
        }
    }

    pub(crate) fn malformed_track() -> Self {
        Self {
            status_code: PublishDoneCode::MalformedTrack,
            error_reason: "malformed track".to_string(),
        }
    }

    /// draft-18 §10.9.1: a REQUEST_UPDATE that failed ends its subscription.
    pub(crate) fn update_failed() -> Self {
        Self {
            status_code: PublishDoneCode::UpdateFailed,
            error_reason: "subscription update failed".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpstreamSubscriptionOrigin {
    Publish,
    Subscribe,
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
        let mut older = unknown_track_properties(1);
        older.other.push(KeyValuePair {
            key: 0x3A,
            value: VariantType::Even(7),
        });
        older.max_cache_duration = Some(1_000);
        let newer = moqt::TrackProperties {
            max_cache_duration: None,
            other: vec![KeyValuePair {
                key: 0x38,
                value: VariantType::Even(2),
            }],
        };
        let mut upstream_track = UpstreamTrack::default();
        upstream_track
            .subscriptions
            .insert(1, upstream_carrying(older));
        upstream_track
            .subscriptions
            .insert(3, upstream_carrying(newer.clone()));

        // Act
        let track_properties = upstream_track.track_properties();

        // Assert: draft-18 §2.5 forbids merging sets
        assert_eq!(track_properties, newer);
    }
}
