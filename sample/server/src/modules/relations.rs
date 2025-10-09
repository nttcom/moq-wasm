use dashmap::{DashMap, DashSet};
use uuid::Uuid;
type Namespace = String;
type TrackNamespace = String;

pub(crate) struct Relations {
    /**
     * namespace mechanism
     * publish_namespace: room/member
     * subscriber_namespace: room/
     * publish: room/member + video
     * subscribe: room/member/video
     */
    pub(crate) publisher_namespaces: DashMap<Namespace, DashSet<Uuid>>,
    pub(crate) subscriber_namespaces: DashMap<Namespace, DashSet<Uuid>>,
    pub(crate) published_tracks: DashMap<Namespace, DashSet<TrackNamespace>>,
    pub(crate) subscribed_tracks: DashMap<Namespace, DashSet<TrackNamespace>>,
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
