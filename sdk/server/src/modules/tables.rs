use dashmap::{DashMap, DashSet};
use uuid::Uuid;
type Namespace = String;
type TrackNamespace = String;

pub(crate) struct Tables {
    pub(crate) publishers: DashMap<Namespace, DashSet<Uuid>>,
    pub(crate) subscribers: DashMap<Namespace, DashSet<Uuid>>,
    pub(crate) publisher_namespaces: DashMap<Namespace, DashSet<TrackNamespace>>,
    pub(crate) subscriber_namespaces: DashMap<Namespace, DashSet<TrackNamespace>>,
}

impl Tables {
    pub(crate) fn new() -> Self {
        Self {
            publishers: DashMap::new(),
            subscribers: DashMap::new(),
            publisher_namespaces: DashMap::new(),
            subscriber_namespaces: DashMap::new(),
        }
    }
}
