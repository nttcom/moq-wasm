use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;

use crate::{
    config::DiskCacheConfig,
    modules::data_plane::cache::{cached_object::CachedObject, track_cache::TrackCache},
};

use super::cached_object::stream_object_with_payload;

/// A fresh directory under the system temp dir, removed on drop.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "relay-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn config(&self, max_bytes: u64) -> DiskCacheConfig {
        DiskCacheConfig {
            dir: self.0.clone(),
            max_bytes,
        }
    }

    pub(crate) fn segment_count(&self) -> usize {
        std::fs::read_dir(&self.0).unwrap().count()
    }

    pub(crate) async fn wait_until_no_segments(&self) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.segment_count() > 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the disk cache should delete the segments");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A cache without a disk holding one in-memory object with `payload`.
pub(crate) fn cache_with_object(payload: &'static [u8]) -> (Arc<TrackCache>, CachedObject) {
    let cache = Arc::new(TrackCache::new());
    let object = stream_object_with_payload(0, 0, Bytes::from_static(payload));
    cache.insert(object.clone()).unwrap();
    (cache, object)
}

pub(crate) async fn wait_until_stored(cache: &TrackCache, at: moqt::object::Location) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !cache.has_stored_payload(at) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the disk cache should store the payload");
}
