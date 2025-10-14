use dashmap::{DashMap, DashSet};

use crate::modules::types::{
    SessionId, TrackName, TrackNamespace, TrackNamespacePrefix, TrackNamespaceWithTrackName,
};

pub(crate) struct Relations {
    /**
     * namespace mechanism
     * publish_namespace: room/member
     * subscriber_namespace: room/
     * publish: room/member + video
     * subscribe: room/member/video
     */
    pub(crate) publisher_namespaces: DashMap<TrackNamespace, DashSet<SessionId>>,
    pub(crate) subscriber_namespaces: DashMap<TrackNamespacePrefix, DashSet<SessionId>>,
    pub(crate) published_tracks: DashMap<TrackNamespace, DashSet<TrackName>>,
    pub(crate) subscribed_tracks: DashMap<TrackNamespaceWithTrackName, DashSet<TrackName>>,
}

impl Relations {
    pub(crate) fn new() -> Self {
        Self {
            publisher_namespaces: DashMap::new(),
            subscriber_namespaces: DashMap::new(),
            published_tracks: DashMap::new(),
            subscribed_tracks: DashMap::new(),
        }
    }
}
