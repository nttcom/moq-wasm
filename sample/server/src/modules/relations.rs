use dashmap::{DashMap, DashSet};
use tokio::sync::RwLock;

use crate::modules::{
    core::handler::publish::PublishHandler,
    types::{SessionId, TrackNamespace, TrackNamespacePrefix},
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
    pub(crate) published_handlers: RwLock<Vec<Box<dyn PublishHandler>>>,
}

impl Relations {
    pub(crate) fn new() -> Self {
        Self {
            publisher_namespaces: DashMap::new(),
            subscriber_namespaces: DashMap::new(),
            published_handlers: RwLock::new(Vec::new()),
        }
    }
}
