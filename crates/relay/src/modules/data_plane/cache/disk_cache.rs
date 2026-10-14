mod disk_cache_task;
mod stored_payload;

use std::{
    path::Path,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use bytes::Bytes;
use tokio::{sync::mpsc, time::Instant};

use crate::modules::{
    data_plane::cache::{cached_object::CachedObject, track_cache::TrackCache},
    domain::track_key::TrackKey,
};

pub(crate) use disk_cache_task::DiskCacheTask;
pub(crate) use stored_payload::StoredPayload;

const SEGMENT_EXTENSION: &str = "seg";

/// Removes the segments a previous run left behind; payloads are not kept
/// across restarts. Only `*.seg` files directly in `dir` are touched, so a
/// misconfigured directory loses nothing else.
pub(crate) async fn prepare_directory(dir: &Path) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == SEGMENT_EXTENSION)
            && entry.file_type().await?.is_file()
        {
            tokio::fs::remove_file(&path).await?;
        }
    }
    Ok(())
}

enum DiskCommand {
    Write {
        track_key: TrackKey,
        cache: Weak<TrackCache>,
        location: moqt::object::Location,
        payload: Bytes,
        retained_until: Instant,
    },
}

/// Hands payloads to the `DiskCacheTask`, the only writer of the directory.
#[derive(Clone)]
pub(crate) struct DiskCache {
    command_sender: mpsc::Sender<DiskCommand>,
    stored_bytes: Arc<AtomicU64>,
    dropped_writes: Arc<AtomicU64>,
}

/// A track's link to the disk cache, held by its `TrackCache`.
pub(crate) struct DiskSink {
    pub(crate) disk: DiskCache,
    pub(crate) track_key: TrackKey,
    /// Weak so that the store can still drop a drained track.
    pub(crate) cache: Weak<TrackCache>,
}

impl DiskSink {
    /// Never waits: when the task falls behind, the object stays in memory
    /// only and leaves the cache at the memory TTL.
    pub(crate) fn write(&self, object: &CachedObject) {
        if object.object.payload.is_empty() {
            return;
        }
        let command = DiskCommand::Write {
            track_key: self.track_key.clone(),
            cache: self.cache.clone(),
            location: object.object.location,
            payload: object.object.payload.clone(),
            retained_until: object.retained_until(),
        };
        if self.disk.command_sender.try_send(command).is_err() {
            self.disk.dropped_writes.fetch_add(1, Ordering::Relaxed);
        }
    }
}
