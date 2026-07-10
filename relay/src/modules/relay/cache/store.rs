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

    pub(crate) async fn evict(&self, ttl: Duration) {
        // Snapshot handles so per-track eviction runs without holding a shard lock.
        let entries: Vec<(TrackKey, Arc<TrackCache>)> = self
            .caches
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        for (_, track) in &entries {
            track.evict(ttl).await;
        }
        // Drop the snapshot Arcs before the strong_count check so it counts only real holders.
        let mut stale_keys = Vec::with_capacity(entries.len());
        for (key, track) in entries {
            stale_keys.push((key, track.is_stale(ttl).await));
        }
        for (key, stale) in stale_keys {
            self.caches
                .remove_if(&key, |_, track| stale && Arc::strong_count(track) == 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn evict_removes_unreferenced_track() {
        // Arrange: a track held only by the store (the returned Arc is dropped immediately)
        let store = TrackCacheStore::new();
        let key = TrackKey::new("ns", "track");
        store.get_or_create(&key);
        // Act
        tokio::time::advance(Duration::from_secs(11)).await;
        store.evict(Duration::from_secs(10)).await;
        // Assert: strong_count == 1 and the track is stale, so it is reclaimed
        assert!(store.get(&key).is_none());
    }

    #[tokio::test]
    async fn evict_keeps_recent_unreferenced_track() {
        // Arrange: a recently written track held only by the store.
        let store = TrackCacheStore::new();
        let key = TrackKey::new("ns", "track");
        store.get_or_create(&key);
        // Act
        store.evict(Duration::from_secs(10)).await;
        // Assert: recent fetch-filled tracks survive long enough for a second FETCH.
        assert!(store.get(&key).is_some());
    }

    #[tokio::test]
    async fn evict_keeps_referenced_track() {
        // Arrange: a track someone else still holds (simulating an active ingress/egress)
        let store = TrackCacheStore::new();
        let key = TrackKey::new("ns", "track");
        let _held = store.get_or_create(&key);
        // Act
        store.evict(Duration::from_secs(10)).await;
        // Assert: strong_count > 1, so the track survives (new-join race avoidance)
        assert!(store.get(&key).is_some());
    }
}
