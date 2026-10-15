use std::time::Duration;

use bytes::Bytes;
use moqt::object::ObjectStatus;

use crate::modules::{
    data_plane::cache::{
        cached_object::{CacheStamp, RETENTION_LIMIT},
        disk_cache::DiskCacheTask,
        store::TrackCacheStore,
        track_cache::FetchRangeResolution,
    },
    domain::track_key::TrackKey,
    test_support::relay_harness::{
        FetchSent, RelayHarness,
        fixtures::{
            data_object::make_status_object,
            disk_cache::{TempDir, wait_until_stored},
            location,
        },
    },
};

const PERSIST: CacheStamp = CacheStamp {
    max_cache_duration: None,
    persist: true,
};

struct DiskBackedTrack {
    harness: RelayHarness,
    store: TrackCacheStore,
    _disk: DiskCacheTask,
    dir: TempDir,
}

fn disk_backed_track(name: &str, max_bytes: u64, stamp: CacheStamp) -> DiskBackedTrack {
    let dir = TempDir::new(name);
    let disk = DiskCacheTask::run(dir.config(max_bytes));
    let store = TrackCacheStore::with_disk(Some(disk.handle()));
    let cache = store.get_or_create(&TrackKey::new("ns", "track"));
    DiskBackedTrack {
        harness: RelayHarness::with_cache(cache, stamp),
        store,
        _disk: disk,
        dir,
    }
}

/// Group `group_id` with one object holding `payload`, closed upstream.
async fn ingest_group(harness: &RelayHarness, group_id: u64, payload: &'static [u8]) {
    let stream = harness.open_upstream_stream();
    stream.header(group_id);
    stream.object_with_payload(Bytes::from_static(payload));
    stream.fin();
    harness.wait_group_closed(group_id).await;
}

/// Lets the objects age past a zero memory TTL, then evicts.
async fn evict_past_memory_ttl(harness: &RelayHarness) {
    tokio::time::sleep(Duration::from_millis(5)).await;
    harness.cache().evict(Duration::ZERO);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetch_reads_a_persisted_payload_back_from_disk_after_the_memory_ttl() {
    // Arrange
    let track = disk_backed_track("fetch-from-disk", u64::MAX, PERSIST);
    ingest_group(&track.harness, 0, b"from disk").await;
    wait_until_stored(track.harness.cache(), location(0, 0)).await;

    // Act
    evict_past_memory_ttl(&track.harness).await;
    let mut fetch = track.harness.start_fetch(location(0, 0), location(0, 1));

    // Assert: the payload left memory and still reaches the subscriber unchanged
    assert_eq!(track.harness.cache().occupancy().payload_bytes, 0);
    let object = fetch.expect_object().await;
    assert_eq!(object.payload, Bytes::from_static(b"from disk"));
    assert!(matches!(fetch.expect_end().await, FetchSent::Closed));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_track_not_stamped_to_persist_leaves_at_the_memory_ttl() {
    // Arrange: a downstream relay's ingest, its upstream being another relay
    let track = disk_backed_track("not-persisted", u64::MAX, CacheStamp::default());
    ingest_group(&track.harness, 0, b"memory only").await;

    // Act
    evict_past_memory_ttl(&track.harness).await;

    // Assert
    assert!(track.harness.cache().is_empty());
    assert_eq!(track.dir.segment_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_end_of_group_of_a_persisted_track_stays_without_a_disk_copy() {
    // Arrange: an empty End of Group status object closes the group
    let track = disk_backed_track("end-of-group", u64::MAX, PERSIST);
    let stream = track.harness.open_upstream_stream();
    stream.header(0);
    stream.object_with_payload(Bytes::from_static(b"object"));
    stream.send(make_status_object(0, 1, ObjectStatus::EndOfGroup));
    track.harness.wait_group_closed(0).await;
    wait_until_stored(track.harness.cache(), location(0, 0)).await;

    // Act
    evict_past_memory_ttl(&track.harness).await;

    // Assert: the whole group is still served from the cache
    let mut fetch = track.harness.start_fetch(location(0, 0), location(0, 0));
    fetch.expect_objects(&[(0, 0), (0, 1)]).await;
    assert!(matches!(fetch.expect_end().await, FetchSent::Closed));
}

/// Two groups whose payloads share one segment, which the byte cap deleted
/// when the second payload went over it.
async fn ingest_past_the_byte_cap(track: &DiskBackedTrack) {
    ingest_group(&track.harness, 0, b"first!").await;
    ingest_group(&track.harness, 1, b"second").await;
    // The cap is enforced right after the second write is recorded.
    wait_until_stored(track.harness.cache(), location(1, 0)).await;
    track.dir.wait_until_no_segments().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fetch_over_a_deleted_segment_goes_upstream() {
    // Arrange
    let track = disk_backed_track("byte-cap-fetch", 6, PERSIST);
    ingest_past_the_byte_cap(&track).await;

    // Act
    let resolution = track
        .harness
        .cache()
        .resolve_fetch_range(location(0, 0), location(0, 1));

    // Assert
    assert_eq!(resolution, FetchRangeResolution::NotCovered);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn objects_whose_segment_the_byte_cap_deleted_leave_the_cache() {
    // Arrange
    let track = disk_backed_track("byte-cap-evict", 6, PERSIST);
    ingest_past_the_byte_cap(&track).await;

    // Act
    track.harness.cache().evict(Duration::from_secs(60));

    // Assert
    assert!(track.harness.cache().is_empty());
}

// current_thread: `tokio::time::pause` needs it.
#[tokio::test]
async fn a_persisted_object_leaves_the_cache_at_the_retention_limit() {
    // Arrange
    let track = disk_backed_track("retention-limit", u64::MAX, PERSIST);
    ingest_group(&track.harness, 0, b"an hour").await;
    wait_until_stored(track.harness.cache(), location(0, 0)).await;
    evict_past_memory_ttl(&track.harness).await;
    tokio::time::pause();
    tokio::time::advance(RETENTION_LIMIT).await;

    // Act
    track.harness.cache().evict(Duration::ZERO);

    // Assert
    assert!(track.harness.cache().is_empty());
}

// current_thread: `tokio::time::pause` needs it.
#[tokio::test]
async fn a_track_whose_publisher_left_loses_its_persisted_objects_after_the_grace_period() {
    // Arrange
    let track = disk_backed_track("publisher-left", u64::MAX, PERSIST);
    ingest_group(&track.harness, 0, b"ended").await;
    wait_until_stored(track.harness.cache(), location(0, 0)).await;
    evict_past_memory_ttl(&track.harness).await;
    track.harness.cache().publisher_left();
    track.harness.cache().evict(Duration::from_secs(60));
    let kept_during_grace = !track.harness.cache().is_empty();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(10)).await;

    // Act
    track.harness.cache().evict(Duration::from_secs(60));

    // Assert
    assert!(kept_during_grace);
    assert!(track.harness.cache().is_empty());
    tokio::time::resume();
    track.dir.wait_until_no_segments().await;
}

// current_thread: `tokio::time::pause` needs it.
#[tokio::test]
async fn a_publisher_returning_within_the_grace_period_keeps_the_persisted_objects() {
    // Arrange
    let track = disk_backed_track("publisher-returned", u64::MAX, PERSIST);
    ingest_group(&track.harness, 0, b"resumed").await;
    wait_until_stored(track.harness.cache(), location(0, 0)).await;
    track.harness.cache().publisher_left();
    track.harness.cache().begin_live_ingest();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(10)).await;

    // Act
    track.harness.cache().evict(Duration::from_secs(60));

    // Assert
    assert_eq!(
        track.harness.cache().largest_location(),
        Some(location(0, 0))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn store_occupancy_reports_the_bytes_on_disk() {
    // Arrange
    let track = disk_backed_track("occupancy", u64::MAX, PERSIST);

    // Act
    ingest_group(&track.harness, 0, b"nine byte").await;

    // Assert
    tokio::time::timeout(Duration::from_secs(3), async {
        while track.store.occupancy().disk_payload_bytes != 9 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the stored bytes should be reported");
}

#[tokio::test]
async fn store_occupancy_reports_the_disk_cap() {
    // Arrange
    let track = disk_backed_track("cap", 5 * 1024, PERSIST);

    // Act
    let occupancy = track.store.occupancy();

    // Assert
    assert_eq!(occupancy.disk_max_bytes, 5 * 1024);
}
