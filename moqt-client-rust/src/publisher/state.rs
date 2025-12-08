use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct PublisherState {
    pub subscribed_aliases: HashSet<u64>,
    pub tracks: HashMap<TrackKey, TrackEntry>,
}

#[derive(Hash, Eq, PartialEq, Debug, Clone)]
pub struct TrackKey {
    pub namespace: String,
    pub name: String,
}

impl TrackKey {
    pub fn new(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct TrackEntry {
    pub alias: u64,
    pub subscriber_alias: Option<u64>,
}
