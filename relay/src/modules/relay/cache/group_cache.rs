use std::{collections::BTreeMap, sync::Arc};

use tokio::{
    sync::{Notify, RwLock},
    time::{Duration, Instant},
};

use crate::modules::core::data_object::DataObject;

pub(crate) struct GroupCache {
    header: RwLock<Option<Arc<DataObject>>>,
    objects: RwLock<BTreeMap<u64, (Instant, Arc<DataObject>)>>,
    end_of_group: RwLock<bool>,
    notify: Notify,
}

impl GroupCache {
    pub(crate) fn new() -> Self {
        Self {
            header: RwLock::new(None),
            objects: RwLock::new(BTreeMap::new()),
            end_of_group: RwLock::new(false),
            notify: Notify::new(),
        }
    }

    // FIXME: §8.1 also requires treating a duplicate whose payload/subgroup/priority
    // differs (or an invalid status transition) as Malformed (MUST). Not implemented;
    // we only ignore duplicates (first-wins).
    pub(crate) async fn append(&self, object_id: Option<u64>, object: Arc<DataObject>) {
        match object_id {
            // No object_id = subgroup header. Keep the first one (first-wins).
            None => {
                let mut header = self.header.write().await;
                if header.is_none() {
                    *header = Some(object);
                }
            }
            // A later object with the same object_id is ignored (draft-14 §8.1 dedup).
            Some(object_id) => {
                self.objects
                    .write()
                    .await
                    .entry(object_id)
                    .or_insert((Instant::now(), object));
            }
        }
        self.notify.notify_waiters();
    }

    pub(crate) async fn evict_expired_objects(&self, ttl: Duration) {
        self.objects
            .write()
            .await
            .retain(|_, (inserted, _)| inserted.elapsed() <= ttl);
    }

    pub(crate) async fn is_evictable(&self) -> bool {
        self.is_closed().await && self.objects.read().await.is_empty()
    }

    pub(crate) async fn header(&self) -> Option<Arc<DataObject>> {
        self.header.read().await.clone()
    }

    pub(crate) async fn largest_object_id(&self) -> Option<u64> {
        self.objects
            .read()
            .await
            .last_key_value()
            .map(|(&id, _)| id)
    }

    /// Waits until the subgroup header is available, or returns `None` if the group is
    /// closed before it arrives.
    pub(crate) async fn header_or_wait(&self) -> Option<Arc<DataObject>> {
        loop {
            // Create notified() before the check to avoid a race between it and the wait.
            let notified = self.notify.notified();
            if let Some(header) = self.header().await {
                return Some(header);
            }
            if self.is_closed().await {
                return None;
            }
            notified.await;
        }
    }

    /// Returns the first object whose id is `>= object_id` (inclusive), waiting if none is
    /// available yet. Returns `None` once the group is closed and no such object exists.
    pub(crate) async fn object_from_or_wait(
        &self,
        object_id: u64,
    ) -> Option<(u64, Arc<DataObject>)> {
        loop {
            // Create notified() before the check to avoid a race between it and the wait.
            let notified = self.notify.notified();
            if let Some((&id, (_, object))) = self.objects.read().await.range(object_id..).next() {
                return Some((id, object.clone()));
            }
            if self.is_closed().await {
                return None;
            }
            notified.await;
        }
    }

    pub(crate) async fn mark_end_of_group(&self) {
        let mut end_of_group = self.end_of_group.write().await;
        *end_of_group = true;
        drop(end_of_group);
        self.notify.notify_waiters();
    }

    pub(crate) async fn is_closed(&self) -> bool {
        *self.end_of_group.read().await
    }

    /// Returns all cached objects in object_id order. The subgroup header is not included.
    /// Production reads go through `object_from_or_wait`; a snapshot can miss
    /// objects still in flight.
    #[cfg(test)]
    pub(crate) async fn objects_snapshot(&self) -> Vec<(u64, Arc<DataObject>)> {
        self.objects
            .read()
            .await
            .iter()
            .map(|(&id, (_, object))| (id, object.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use moqt::{ExtensionHeaders, SubgroupHeader, SubgroupId, SubgroupObject, SubgroupObjectField};
    use std::time::Duration;

    fn payload_object(payload: &[u8]) -> Arc<DataObject> {
        let message_type =
            SubgroupHeader::new(0, 0, SubgroupId::Value(0), 0, false, false).message_type;
        Arc::new(DataObject::SubgroupObject(SubgroupObjectField {
            message_type,
            object_id_delta: 0,
            extension_headers: ExtensionHeaders {
                prior_group_id_gap: vec![],
                prior_object_id_gap: vec![],
                immutable_extensions: vec![],
            },
            subgroup_object: SubgroupObject::new_payload(Bytes::from(payload.to_vec())),
        }))
    }

    fn payload_of(object: &DataObject) -> Bytes {
        match object {
            DataObject::SubgroupObject(f) => match &f.subgroup_object {
                SubgroupObject::Payload { data, .. } => data.clone(),
                _ => panic!("expected payload"),
            },
            _ => panic!("expected SubgroupObject"),
        }
    }

    fn header_object(publisher_priority: u8) -> Arc<DataObject> {
        Arc::new(DataObject::SubgroupHeader(SubgroupHeader::new(
            0,
            0,
            SubgroupId::Value(0),
            publisher_priority,
            false,
            false,
        )))
    }

    #[tokio::test]
    async fn append_same_object_id_keeps_first() {
        // Arrange: two objects with the same object_id but distinct payloads
        let cache = GroupCache::new();
        cache.append(Some(0), payload_object(b"first")).await;
        cache.append(Some(0), payload_object(b"second")).await;
        // Act
        let snapshot = cache.objects_snapshot().await;
        // Assert: only the first object survives (first-wins dedup)
        assert_eq!(snapshot.len(), 1);
        let (id, object) = &snapshot[0];
        assert_eq!(*id, 0);
        assert_eq!(payload_of(object), Bytes::from_static(b"first"));
    }

    #[tokio::test]
    async fn append_header_twice_keeps_first() {
        // Arrange: two subgroup headers distinguished by publisher_priority
        let cache = GroupCache::new();
        cache.append(None, header_object(1)).await;
        cache.append(None, header_object(2)).await;
        // Act
        let header = cache.header().await.unwrap();
        // Assert: the first header survives (first-wins)
        match header.as_ref() {
            DataObject::SubgroupHeader(h) => assert_eq!(h.publisher_priority, 1),
            _ => panic!("expected SubgroupHeader"),
        }
    }

    #[tokio::test]
    async fn object_from_or_wait_returns_exact_match() {
        // Arrange: objects at ids 0, 3, 5
        let cache = GroupCache::new();
        cache.append(Some(0), payload_object(b"o0")).await;
        cache.append(Some(3), payload_object(b"o3")).await;
        cache.append(Some(5), payload_object(b"o5")).await;
        // Act: ask for exactly id 3
        let (id, _) = cache.object_from_or_wait(3).await.unwrap();
        // Assert: the exact id is returned (inclusive lower bound)
        assert_eq!(id, 3);
    }

    #[tokio::test]
    async fn object_from_or_wait_skips_gap_to_next_id() {
        // Arrange: objects at ids 0, 3, 5 (no object at 1, 2, 4)
        let cache = GroupCache::new();
        cache.append(Some(0), payload_object(b"o0")).await;
        cache.append(Some(3), payload_object(b"o3")).await;
        cache.append(Some(5), payload_object(b"o5")).await;
        // Act: ask for id 4, which has no object
        let (id, _) = cache.object_from_or_wait(4).await.unwrap();
        // Assert: the next available id (5) is returned across the gap
        assert_eq!(id, 5);
    }

    #[tokio::test(start_paused = true)]
    async fn evict_removes_objects_older_than_ttl() {
        // Arrange: object 0 at t=0, object 1 at t=6s, TTL=10s
        let ttl = Duration::from_secs(10);
        let cache = GroupCache::new();
        cache.append(Some(0), payload_object(b"old")).await;
        tokio::time::advance(Duration::from_secs(6)).await;
        cache.append(Some(1), payload_object(b"new")).await;
        // Act: at t=11s, object 0 is 11s old (>10), object 1 is 5s old (<=10)
        tokio::time::advance(Duration::from_secs(5)).await;
        cache.evict_expired_objects(ttl).await;
        // Assert: only the expired object 0 is removed
        let snapshot = cache.objects_snapshot().await;
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].0, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn evict_keeps_object_at_exactly_ttl() {
        // Arrange: object 0 at t=0, TTL=10s
        let ttl = Duration::from_secs(10);
        let cache = GroupCache::new();
        cache.append(Some(0), payload_object(b"o")).await;
        // Act: at exactly t=10s, age == TTL, which is not `> TTL`
        tokio::time::advance(Duration::from_secs(10)).await;
        cache.evict_expired_objects(ttl).await;
        // Assert: the boundary object survives
        assert_eq!(cache.objects_snapshot().await.len(), 1);
    }

    #[tokio::test]
    async fn object_from_or_wait_returns_none_when_closed() {
        // Arrange: one object at id 0, then the group is closed
        let cache = GroupCache::new();
        cache.append(Some(0), payload_object(b"o0")).await;
        cache.mark_end_of_group().await;
        // Act: ask for id 1, beyond the last object, on a closed group
        let result = cache.object_from_or_wait(1).await;
        // Assert: no object exists and the group is closed, so None
        assert!(result.is_none());
    }
}
