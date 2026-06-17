use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{Mutex, OwnedMutexGuard};

type TrackLockMap = DashMap<(String, String), Arc<Mutex<()>>>;

/// Per-`(track_namespace, track_name)` async mutex map.
///
/// Callers acquire an owned guard for a given track key before calling
/// `create_upstream_subscription`. While the guard is held, any other
/// concurrent task trying to acquire the same key will wait. Different
/// track keys use independent locks and do not block each other.
///
/// # Entry lifecycle
/// Entries are left in place after the guard is dropped (option b).
/// The map grows by the number of distinct tracks seen in this relay
/// instance and can be GC-ed as a follow-up if needed.
/// NOTE: for typical MoQT deployments the number of distinct tracks is
/// bounded and small, so growth is not a concern in practice.
#[derive(Clone, Debug, Default)]
pub(crate) struct UpstreamCreationSerializer {
    locks: Arc<TrackLockMap>,
}

impl UpstreamCreationSerializer {
    pub(crate) fn new() -> Self {
        Self {
            locks: Arc::new(DashMap::new()),
        }
    }

    /// Acquire an exclusive async guard for the given track key.
    /// Returns an owned guard; dropping the guard releases the lock.
    pub(crate) async fn lock(
        &self,
        track_namespace: &str,
        track_name: &str,
    ) -> OwnedMutexGuard<()> {
        let key = (track_namespace.to_owned(), track_name.to_owned());
        let mutex = self
            .locks
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        mutex.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use super::UpstreamCreationSerializer;

    /// Two tasks contending on the same key execute serially, not concurrently.
    /// We verify this by checking a shared counter: the second task must see
    /// the counter already incremented by the first.
    #[tokio::test]
    async fn same_key_tasks_serialize() {
        let serializer = UpstreamCreationSerializer::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let s1 = serializer.clone();
        let c1 = counter.clone();
        let t1 = tokio::spawn(async move {
            let _guard = s1.lock("ns", "track").await;
            let before = c1.fetch_add(1, Ordering::SeqCst);
            // yield so the other task can try to acquire
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
            let after = c1.load(Ordering::SeqCst);
            // while we hold the guard no other task should have incremented
            (before, after)
        });

        // Give t1 time to acquire the lock before t2 starts.
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

        let s2 = serializer.clone();
        let c2 = counter.clone();
        let t2 = tokio::spawn(async move {
            let _guard = s2.lock("ns", "track").await;
            c2.fetch_add(1, Ordering::SeqCst)
        });

        let (before1, after1) = t1.await.unwrap();
        let before2 = t2.await.unwrap();

        // t1 started with counter=0, saw counter=1 after sleep (still 1, t2 blocked)
        assert_eq!(before1, 0, "t1 should be first");
        assert_eq!(after1, 1, "t2 must not have run while t1 held the lock");
        // t2 acquired after t1 released, so it saw counter=1 before its own add
        assert_eq!(before2, 1, "t2 should run after t1");
    }

    /// Tasks on different keys must NOT block each other.
    #[tokio::test]
    async fn different_keys_run_concurrently() {
        let serializer = UpstreamCreationSerializer::new();

        let s1 = serializer.clone();
        let t1 = tokio::spawn(async move {
            let _guard = s1.lock("ns", "track-a").await;
            tokio::time::sleep(tokio::time::Duration::from_millis(40)).await;
            Instant::now()
        });

        let s2 = serializer.clone();
        let t2 = tokio::spawn(async move {
            let _guard = s2.lock("ns", "track-b").await;
            tokio::time::sleep(tokio::time::Duration::from_millis(40)).await;
            Instant::now()
        });

        let start = Instant::now();
        let (end1, end2) = tokio::join!(t1, t2);
        let elapsed = start.elapsed();

        // Both tasks hold their locks simultaneously for ~40 ms each;
        // total wall time should be well under 70 ms (not 80+).
        assert!(
            elapsed.as_millis() < 70,
            "different keys should not block each other, elapsed={elapsed:?}"
        );
        let _ = (end1, end2);
    }
}
