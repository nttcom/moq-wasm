use std::{sync::Arc, time::Duration};

use dashmap::DashMap;

use crate::modules::{relay::cache::track_cache::TrackCache, types::TrackKey};

pub(crate) struct TrackCacheStore {
    caches: DashMap<TrackKey, Arc<TrackCache>>,
}

impl TrackCacheStore {
    pub(crate) fn new() -> Self {
        Self {
            caches: DashMap::new(),
        }
    }

    pub(crate) fn get(&self, track_key: &TrackKey) -> Option<Arc<TrackCache>> {
        // clone the Arc to drop the Ref and release the DashMap shard lock
        self.caches.get(track_key).map(|v| v.clone())
    }

    pub(crate) fn get_or_create(&self, track_key: &TrackKey) -> Arc<TrackCache> {
        self.caches
            .entry(track_key.clone())
            .or_insert_with(|| Arc::new(TrackCache::new()))
            .clone()
    }

    pub(crate) fn evict(&self, ttl: Duration) {
        // Snapshot handles so per-track eviction runs without holding a shard lock.
        let entries: Vec<(TrackKey, Arc<TrackCache>)> = self
            .caches
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        // Consuming the snapshot drops each Arc with its iteration, so the
        // strong_count check below sees only real holders.
        let mut empty_keys = Vec::new();
        for (key, track) in entries {
            track.evict(ttl);
            if track.is_empty() {
                empty_keys.push(key);
            }
        }
        // Only TTL-drained (empty) tracks may be removed: an unreferenced track
        // must keep serving FETCH until then. Both guards are re-checked under
        // the shard lock because a session may attach, write, and detach in the
        // gap since the loop above: with strong_count == 1 no other handle can
        // write through, and the emptiness re-check keeps objects written in
        // that gap.
        for key in empty_keys {
            self.caches.remove_if(&key, |_, track| {
                Arc::strong_count(track) == 1 && track.is_empty()
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::relay::tests::harness::fixtures::cached_object::insert_closed_live_group;
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn evict_removes_unreferenced_track() {
        // Arrange: a track held only by the store (the returned Arc is dropped immediately)
        let store = TrackCacheStore::new();
        let key = TrackKey::new("ns", "track");
        store.get_or_create(&key);
        // Act
        store.evict(Duration::from_secs(10));
        // Assert: strong_count == 1 and the track is empty, so it is reclaimed
        assert!(store.get(&key).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn evict_keeps_unreferenced_track_with_fresh_objects() {
        // Arrange: a publisher ingested one closed group and disconnected, so
        // only the store holds the track (strong_count == 1).
        let ttl = Duration::from_secs(30);
        let store = TrackCacheStore::new();
        let key = TrackKey::new("ns", "track");
        insert_closed_live_group(&store.get_or_create(&key), 0, &[0]);

        // Act / Assert: within the TTL the track must survive to serve FETCH.
        tokio::time::advance(Duration::from_secs(1)).await;
        store.evict(ttl);
        assert!(store.get(&key).is_some());

        // Act / Assert: once the objects drain past the TTL, the track is reclaimed.
        tokio::time::advance(Duration::from_secs(31)).await;
        store.evict(ttl);
        assert!(store.get(&key).is_none());
    }

    #[tokio::test]
    async fn evict_keeps_referenced_track() {
        // Arrange: a track someone else still holds (simulating an active ingress/egress)
        let store = TrackCacheStore::new();
        let key = TrackKey::new("ns", "track");
        let _held = store.get_or_create(&key);
        // Act
        store.evict(Duration::from_secs(10));
        // Assert: strong_count > 1, so the track survives (new-join race avoidance)
        assert!(store.get(&key).is_some());
    }
}
