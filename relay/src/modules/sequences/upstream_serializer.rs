use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{Mutex, OwnedMutexGuard};

type TrackLockKey = (String, String);
type TrackLockMap = DashMap<TrackLockKey, Arc<Mutex<()>>>;

#[derive(Clone, Debug, Default)]
pub(crate) struct UpstreamCreationSerializer {
    locks: Arc<TrackLockMap>,
}

pub(crate) struct UpstreamCreationGuard {
    mutex_guard: Option<OwnedMutexGuard<()>>,
    key: TrackLockKey,
    locks: Arc<TrackLockMap>,
}

impl Drop for UpstreamCreationGuard {
    fn drop(&mut self) {
        drop(self.mutex_guard.take());
        // A waiter holds its own clone of the mutex, so the entry is only removed when nobody
        // waits on it; a later lock() then creates a fresh mutex no one else can hold.
        self.locks
            .remove_if(&self.key, |_, mutex| Arc::strong_count(mutex) == 1);
    }
}

impl UpstreamCreationSerializer {
    pub(crate) async fn lock(
        &self,
        track_namespace: &str,
        track_name: &str,
    ) -> UpstreamCreationGuard {
        let key = (track_namespace.to_owned(), track_name.to_owned());
        let mutex = self
            .locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        UpstreamCreationGuard {
            mutex_guard: Some(mutex.lock_owned().await),
            key,
            locks: self.locks.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use super::{TrackLockKey, UpstreamCreationSerializer};

    fn key() -> TrackLockKey {
        ("ns".to_string(), "track".to_string())
    }

    #[tokio::test]
    async fn same_key_tasks_serialize() {
        // Arrange
        let serializer = UpstreamCreationSerializer::default();
        let counter = Arc::new(AtomicUsize::new(0));

        // Act: t1 holds the lock across a sleep while t2 tries to take it.
        let s1 = serializer.clone();
        let c1 = counter.clone();
        let t1 = tokio::spawn(async move {
            let _guard = s1.lock("ns", "track").await;
            let before = c1.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
            let after = c1.load(Ordering::SeqCst);
            (before, after)
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        let s2 = serializer.clone();
        let c2 = counter.clone();
        let t2 = tokio::spawn(async move {
            let _guard = s2.lock("ns", "track").await;
            c2.fetch_add(1, Ordering::SeqCst)
        });
        let (before1, after1) = t1.await.unwrap();
        let before2 = t2.await.unwrap();

        // Assert
        assert_eq!(before1, 0, "t1 should be first");
        assert_eq!(after1, 1, "t2 must not have run while t1 held the lock");
        assert_eq!(before2, 1, "t2 should run after t1");
    }

    #[tokio::test]
    async fn different_keys_run_concurrently() {
        // Arrange
        let serializer = UpstreamCreationSerializer::default();

        // Act: each task holds its own key's lock for 40 ms.
        let s1 = serializer.clone();
        let t1 = tokio::spawn(async move {
            let _guard = s1.lock("ns", "track-a").await;
            tokio::time::sleep(tokio::time::Duration::from_millis(40)).await;
        });
        let s2 = serializer.clone();
        let t2 = tokio::spawn(async move {
            let _guard = s2.lock("ns", "track-b").await;
            tokio::time::sleep(tokio::time::Duration::from_millis(40)).await;
        });
        let start = Instant::now();
        let _ = tokio::join!(t1, t2);
        let elapsed = start.elapsed();

        // Assert: serialized tasks would take 80 ms or more.
        assert!(
            elapsed.as_millis() < 70,
            "different keys should not block each other, elapsed={elapsed:?}"
        );
    }

    #[tokio::test]
    async fn released_lock_removes_its_entry() {
        // Arrange
        let serializer = UpstreamCreationSerializer::default();
        let guard = serializer.lock("ns", "track").await;

        // Act
        drop(guard);

        // Assert
        assert!(serializer.locks.is_empty());
    }

    #[tokio::test]
    async fn released_lock_keeps_its_entry_for_a_waiter() {
        // Arrange
        let serializer = UpstreamCreationSerializer::default();
        let guard = serializer.lock("ns", "track").await;
        let waiter = tokio::spawn({
            let serializer = serializer.clone();
            async move { serializer.lock("ns", "track").await }
        });
        while Arc::strong_count(&serializer.locks.get(&key()).unwrap()) < 3 {
            tokio::task::yield_now().await;
        }

        // Act
        drop(guard);
        let _waiter_guard = waiter.await.unwrap();

        // Assert
        assert!(serializer.locks.contains_key(&key()));
    }
}
