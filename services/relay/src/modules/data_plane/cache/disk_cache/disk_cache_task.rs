use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use tokio::{
    io::AsyncWriteExt,
    sync::mpsc,
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
};

use crate::modules::{data_plane::cache::track_cache::TrackCache, domain::track_key::TrackKey};

use crate::config::DiskCacheConfig;

use super::{
    DiskCache, DiskCommand, SEGMENT_EXTENSION,
    stored_payload::{Segment, StoredPayload},
};

const COMMAND_CAPACITY: usize = 4096;
/// A track starts a new segment file this often, so expiry and the byte cap
/// free space in steps of about this much media.
const SEGMENT_SPAN: Duration = Duration::from_secs(60);
const EXPIRY_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) struct DiskCacheTask {
    join_handle: JoinHandle<()>,
    handle: DiskCache,
}

impl DiskCacheTask {
    /// `config.dir` must have gone through `prepare_directory`.
    pub(crate) fn run(config: DiskCacheConfig) -> Self {
        let (command_sender, mut command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let handle = DiskCache {
            command_sender,
            stored_bytes: Arc::new(AtomicU64::new(0)),
            dropped_writes: Arc::new(AtomicU64::new(0)),
            max_bytes: config.max_bytes,
        };
        let mut state = DiskState::new(config, handle.stored_bytes.clone());
        let dropped_writes = handle.dropped_writes.clone();
        let join_handle = tokio::spawn(async move {
            let mut expiry = tokio::time::interval(EXPIRY_INTERVAL);
            expiry.set_missed_tick_behavior(MissedTickBehavior::Delay);
            let mut reported_drops = 0;
            loop {
                tokio::select! {
                    command = command_receiver.recv() => match command {
                        Some(DiskCommand::Write { track_key, cache, location, payload, retained_until }) => {
                            state
                                .write(track_key, cache, location, payload, retained_until, Instant::now())
                                .await;
                        }
                        Some(DiskCommand::PurgeTrack(track_key)) => state.purge(&track_key).await,
                        None => break,
                    },
                    _ = expiry.tick() => {
                        state.expire(Instant::now()).await;
                        let drops = dropped_writes.load(Ordering::Relaxed);
                        if drops != reported_drops {
                            tracing::warn!(
                                dropped = drops - reported_drops,
                                "disk cache fell behind; objects stayed in memory only"
                            );
                            reported_drops = drops;
                        }
                    }
                }
            }
        });
        Self {
            join_handle,
            handle,
        }
    }

    pub(crate) fn handle(&self) -> DiskCache {
        self.handle.clone()
    }
}

impl Drop for DiskCacheTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

struct SegmentFile {
    segment: Arc<Segment>,
    /// Open only while this is the track's newest segment and no write to it
    /// failed.
    file: Option<tokio::fs::File>,
    len: u64,
    opened_at: Instant,
    retained_until: Instant,
}

struct DiskState {
    config: DiskCacheConfig,
    tracks: HashMap<TrackKey, VecDeque<SegmentFile>>,
    /// Shared with `DiskCache` for the stats snapshot.
    stored_bytes: Arc<AtomicU64>,
    next_segment_id: u64,
    last_write_warning: Option<Instant>,
}

impl DiskState {
    fn new(config: DiskCacheConfig, stored_bytes: Arc<AtomicU64>) -> Self {
        Self {
            config,
            tracks: HashMap::new(),
            stored_bytes,
            next_segment_id: 0,
            last_write_warning: None,
        }
    }

    async fn write(
        &mut self,
        track_key: TrackKey,
        cache: Weak<TrackCache>,
        location: moqt::object::Location,
        payload: Bytes,
        retained_until: Instant,
        now: Instant,
    ) {
        let Some(cache) = cache.upgrade() else {
            return;
        };
        match self.append(track_key, &payload, retained_until, now).await {
            Ok(stored) => cache.record_stored(location, stored),
            Err(error) => {
                // A full disk fails every write; one line per 10 s is enough.
                if self
                    .last_write_warning
                    .is_none_or(|at| now.duration_since(at) >= Duration::from_secs(10))
                {
                    tracing::warn!(
                        ?error,
                        "disk cache write failed; object stays in memory only"
                    );
                    self.last_write_warning = Some(now);
                }
            }
        }
        self.enforce_byte_cap().await;
    }

    async fn append(
        &mut self,
        track_key: TrackKey,
        payload: &[u8],
        retained_until: Instant,
        now: Instant,
    ) -> anyhow::Result<StoredPayload> {
        let segments = self.tracks.entry(track_key).or_default();
        let current_is_open = segments.back().is_some_and(|segment| {
            segment.file.is_some() && now.duration_since(segment.opened_at) < SEGMENT_SPAN
        });
        if !current_is_open {
            if let Some(previous) = segments.back_mut() {
                previous.file = None;
            }
            let path = self
                .config
                .dir
                .join(format!("{}.{SEGMENT_EXTENSION}", self.next_segment_id));
            self.next_segment_id += 1;
            let file = tokio::fs::File::create(&path).await?;
            segments.push_back(SegmentFile {
                segment: Arc::new(Segment::new(path)),
                file: Some(file),
                len: 0,
                opened_at: now,
                retained_until,
            });
        }
        let current = segments.back_mut().expect("a segment was just ensured");
        let file = current.file.as_mut().expect("the newest segment is open");
        let written = async {
            file.write_all(payload).await?;
            // Readers open the path themselves; flush hands the bytes to the OS.
            file.flush().await
        }
        .await;
        if let Err(error) = written {
            // The file may hold a partial payload; later objects go to a new one.
            current.file = None;
            return Err(error.into());
        }
        let offset = current.len;
        current.len += payload.len() as u64;
        current.retained_until = current.retained_until.max(retained_until);
        self.stored_bytes
            .fetch_add(payload.len() as u64, Ordering::Relaxed);
        Ok(StoredPayload {
            segment: current.segment.clone(),
            offset,
            len: payload.len(),
        })
    }

    /// Deletes the oldest segments of any track until the total fits.
    async fn enforce_byte_cap(&mut self) {
        while self.stored_bytes() > self.config.max_bytes {
            let oldest = self
                .tracks
                .iter()
                .filter_map(|(key, segments)| segments.front().map(|front| (key, front.opened_at)))
                .min_by_key(|(_, opened_at)| *opened_at)
                .map(|(key, _)| key.clone());
            let Some(key) = oldest else {
                break;
            };
            let segment = self
                .tracks
                .get_mut(&key)
                .and_then(VecDeque::pop_front)
                .expect("the oldest track has a front segment");
            self.delete(segment).await;
            self.tracks.retain(|_, segments| !segments.is_empty());
        }
    }

    async fn expire(&mut self, now: Instant) {
        let mut expired = Vec::new();
        for segments in self.tracks.values_mut() {
            while segments
                .front()
                .is_some_and(|front| front.retained_until <= now)
            {
                expired.extend(segments.pop_front());
            }
        }
        self.tracks.retain(|_, segments| !segments.is_empty());
        for segment in expired {
            self.delete(segment).await;
        }
    }

    async fn purge(&mut self, track_key: &TrackKey) {
        for segment in self.tracks.remove(track_key).unwrap_or_default() {
            self.delete(segment).await;
        }
    }

    async fn delete(&mut self, segment: SegmentFile) {
        segment.segment.mark_deleted();
        self.stored_bytes.fetch_sub(segment.len, Ordering::Relaxed);
        drop(segment.file);
        if let Err(error) = tokio::fs::remove_file(&segment.segment.path).await {
            tracing::warn!(?error, path = %segment.segment.path.display(), "failed to delete disk cache segment");
        }
    }

    fn stored_bytes(&self) -> u64 {
        self.stored_bytes.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::test_support::relay_harness::fixtures::disk_cache::{
        TempDir, cache_with_object,
    };

    fn state(dir: &TempDir, max_bytes: u64) -> DiskState {
        DiskState::new(
            DiskCacheConfig {
                dir: dir.path().to_path_buf(),
                max_bytes,
            },
            Arc::new(AtomicU64::new(0)),
        )
    }

    fn track(name: &str) -> TrackKey {
        TrackKey::new("ns", name)
    }

    const HOUR: Duration = Duration::from_secs(3600);

    #[tokio::test]
    async fn an_appended_payload_reads_back_unchanged() {
        // Arrange
        let dir = TempDir::new("disk-cache-read-back");
        let mut state = state(&dir, u64::MAX);
        let now = Instant::now();
        state
            .append(track("a"), b"first", now + HOUR, now)
            .await
            .unwrap();

        // Act
        let stored = state
            .append(track("a"), b"second", now + HOUR, now)
            .await
            .unwrap();

        // Assert
        assert_eq!(stored.read().await.unwrap(), Bytes::from_static(b"second"));
        assert_eq!(state.stored_bytes(), 11);
    }

    #[tokio::test]
    async fn a_track_starts_a_new_segment_after_the_segment_span() {
        // Arrange
        let dir = TempDir::new("disk-cache-rotation");
        let mut state = state(&dir, u64::MAX);
        let now = Instant::now();
        let first = state
            .append(track("a"), b"1", now + HOUR, now)
            .await
            .unwrap();

        // Act
        let second = state
            .append(track("a"), b"2", now + HOUR, now + SEGMENT_SPAN)
            .await
            .unwrap();

        // Assert
        assert_ne!(first.segment.path, second.segment.path);
        assert_eq!(first.read().await.unwrap(), Bytes::from_static(b"1"));
    }

    #[tokio::test]
    async fn the_byte_cap_deletes_the_oldest_segment_of_any_track() {
        // Arrange: track a's segment is older than track b's
        let dir = TempDir::new("disk-cache-byte-cap");
        let mut state = state(&dir, 8);
        let now = Instant::now();
        let oldest = state
            .append(track("a"), b"aaaa", now + HOUR, now)
            .await
            .unwrap();
        let newer = state
            .append(
                track("b"),
                b"bbbb",
                now + HOUR,
                now + Duration::from_secs(1),
            )
            .await
            .unwrap();
        state
            .append(track("b"), b"cc", now + HOUR, now + Duration::from_secs(2))
            .await
            .unwrap();

        // Act
        state.enforce_byte_cap().await;

        // Assert
        assert!(oldest.is_lost());
        assert!(!oldest.segment.path.exists());
        assert!(!newer.is_lost());
        assert_eq!(state.stored_bytes(), 6);
    }

    #[tokio::test]
    async fn expiry_deletes_a_segment_once_its_last_object_expires() {
        // Arrange
        let dir = TempDir::new("disk-cache-expiry");
        let mut state = state(&dir, u64::MAX);
        let now = Instant::now();
        let stored = state
            .append(track("a"), b"early", now + Duration::from_secs(1), now)
            .await
            .unwrap();
        state
            .append(track("a"), b"late", now + Duration::from_secs(2), now)
            .await
            .unwrap();

        // Act
        state.expire(now + Duration::from_secs(1)).await;
        let kept_until_last_object = !stored.is_lost();
        state.expire(now + Duration::from_secs(2)).await;

        // Assert
        assert!(kept_until_last_object);
        assert!(stored.is_lost());
        assert_eq!(state.stored_bytes(), 0);
        assert!(state.tracks.is_empty());
    }

    #[tokio::test]
    async fn purge_deletes_only_the_segments_of_its_track() {
        // Arrange
        let dir = TempDir::new("disk-cache-purge");
        let mut state = state(&dir, u64::MAX);
        let now = Instant::now();
        let purged = state
            .append(track("a"), b"a", now + HOUR, now)
            .await
            .unwrap();
        let kept = state
            .append(track("b"), b"b", now + HOUR, now)
            .await
            .unwrap();

        // Act
        state.purge(&track("a")).await;

        // Assert
        assert!(purged.is_lost());
        assert!(!kept.is_lost());
        assert_eq!(state.stored_bytes(), 1);
    }

    #[tokio::test]
    async fn a_write_records_the_stored_payload_on_the_track_cache() {
        // Arrange
        let dir = TempDir::new("disk-cache-record");
        let mut state = state(&dir, u64::MAX);
        let (cache, object) = cache_with_object(b"payload");
        let now = Instant::now();

        // Act
        state
            .write(
                track("a"),
                Arc::downgrade(&cache),
                object.object.location,
                object.object.payload.clone(),
                now + HOUR,
                now,
            )
            .await;

        // Assert
        assert!(cache.has_stored_payload(object.object.location));
        assert_eq!(state.stored_bytes(), 7);
    }

    #[tokio::test]
    async fn prepare_directory_removes_only_segment_files() {
        // Arrange
        let dir = TempDir::new("disk-cache-prepare");
        std::fs::write(dir.path().join("0.seg"), b"old").unwrap();
        std::fs::write(dir.path().join("keep.txt"), b"other").unwrap();

        // Act
        super::super::prepare_directory(dir.path()).await.unwrap();

        // Assert
        assert!(!dir.path().join("0.seg").exists());
        assert!(dir.path().join("keep.txt").exists());
    }
}
