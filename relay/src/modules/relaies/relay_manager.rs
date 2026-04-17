use dashmap::DashMap;

use crate::modules::{relaies::relay::Relay, types::SessionId};

pub struct RelayManager {
    pub(crate) relay_map: DashMap<u128, Relay>,
    relay_sessions: DashMap<u128, (SessionId, SessionId)>,
}

impl RelayManager {
    pub(crate) fn new() -> Self {
        Self {
            relay_map: DashMap::new(),
            relay_sessions: DashMap::new(),
        }
    }

    pub(crate) fn insert(
        &self,
        track_key: u128,
        publisher_session_id: SessionId,
        subscriber_session_id: SessionId,
        relay: Relay,
    ) {
        self.relay_sessions
            .insert(track_key, (publisher_session_id, subscriber_session_id));
        self.relay_map.insert(track_key, relay);
    }

    pub(crate) fn remove_session(&self, session_id: SessionId) {
        let track_keys = self
            .relay_sessions
            .iter()
            .filter_map(|entry| {
                let (publisher_session_id, subscriber_session_id) = *entry.value();
                (publisher_session_id == session_id || subscriber_session_id == session_id)
                    .then_some(*entry.key())
            })
            .collect::<Vec<_>>();

        for track_key in track_keys {
            self.relay_sessions.remove(&track_key);
            self.relay_map.remove(&track_key);
        }
    }
}
