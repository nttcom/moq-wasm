use moqt::{
    object::{ForwardingPreference, MalformedTrackError, Object, ObjectStatus},
    publish::PublishDoneCode,
};

use crate::modules::{
    session::session_event::EventKind,
    test_support::relay_harness::{
        PUBLISHER_SESSION_ID, RelayHarness, UpstreamTrack,
        fixtures::{cached_object::status_object, location},
        ordered_payload, payloads_of, receive_objects_until_close,
    },
};

fn empty_datagram(object_id: u64, status: ObjectStatus) -> Object {
    Object {
        forwarding: ForwardingPreference::Datagram,
        ..status_object(0, object_id, status).object
    }
}

/// Sends datagram {0,0} as an empty Normal object, then as End of Group,
/// then datagram {0,1}.
async fn send_a_status_only_duplicate(upstream: &UpstreamTrack) {
    upstream.datagram(empty_datagram(0, ObjectStatus::Normal));
    upstream.datagram(empty_datagram(0, ObjectStatus::EndOfGroup));
    upstream.datagram(empty_datagram(1, ObjectStatus::Normal));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicate_object_with_different_payload_terminates_subscription() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;

    // Act
    let _streams = harness.ingest_conflicting_duplicate();

    // Assert
    let publish_done = egress.expect_publish_done().await;
    assert_eq!(publish_done.status_code, PublishDoneCode::MalformedTrack);
    assert_eq!(publish_done.request_id, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_duplicate_from_second_stream_is_not_malformed() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;

    // Act: a second stream re-delivers object 0 with an identical payload
    let first_stream = harness.open_upstream_stream();
    first_stream.header(0);
    first_stream.object(0);
    let second_stream = harness.open_upstream_stream();
    second_stream.header(0);
    second_stream.object(0);
    first_stream.object(1);
    harness.wait_largest_location(location(0, 1)).await;
    first_stream.fin();
    second_stream.fin();

    // Assert: deduplicated delivery, no PUBLISH_DONE
    let objects = receive_objects_until_close(&mut egress).await;
    assert_eq!(
        payloads_of(&objects),
        vec![ordered_payload(0), ordered_payload(1)]
    );
    egress.assert_no_publish_done();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscription_started_after_detection_is_terminated_immediately() {
    // Arrange: latch the track before any downstream subscriber attaches
    let harness = RelayHarness::new();
    let _streams = harness.ingest_conflicting_duplicate();
    harness.wait_track_malformed().await;

    // Act
    let mut egress = harness.start_egress(None).await;
    let publish_done = egress.expect_publish_done().await;

    // Assert: the runner terminates right away, with Stream Count 0
    assert_eq!(publish_done.status_code, PublishDoneCode::MalformedTrack);
    assert_eq!(publish_done.stream_count, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detection_reports_the_publisher_session_and_track() {
    // Arrange
    let mut harness = RelayHarness::new();

    // Act
    let _streams = harness.ingest_conflicting_duplicate();

    // Assert
    let event = harness.expect_session_event().await;
    assert_eq!(event.session_id, PUBLISHER_SESSION_ID);
    assert!(matches!(
        event.kind,
        EventKind::MalformedTrackDetected(ref track_key) if track_key == harness.track_key()
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_object_with_malformed_immutable_properties_terminates_the_subscription() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let stream = harness.open_upstream_stream();
    stream.header(0);
    stream.object(0);
    harness.wait_largest_location(location(0, 0)).await;

    // Act: moqt withholds object 1 and reports why
    stream.malformed_track(MalformedTrackError::MalformedImmutableProperties {
        location: location(0, 1),
    });

    // Assert: draft-14 §11.2 / draft-18 §12.7 make the track malformed; the
    // object never reaches the cache
    let publish_done = egress.expect_publish_done().await;
    assert_eq!(publish_done.status_code, PublishDoneCode::MalformedTrack);
    assert!(harness.is_track_malformed());
    assert_eq!(harness.cache().occupancy().objects, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_draft14_duplicate_that_changes_only_its_status_latches_the_track() {
    // Arrange
    let harness = RelayHarness::new();
    let upstream = harness.start_track_ingest();

    // Act
    send_a_status_only_duplicate(&upstream).await;

    // Assert: draft-14 §8.1, a move between Normal and End of Group is malformed
    harness.wait_track_malformed().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_draft18_duplicate_that_changes_only_its_status_does_not_latch_the_track() {
    // Arrange
    let harness = RelayHarness::new();
    let upstream = harness.start_draft18_track_ingest();

    // Act
    send_a_status_only_duplicate(&upstream).await;
    harness.wait_largest_location(location(0, 1)).await;

    // Assert: draft-18 §9.1 compares only the forwarding preference, subgroup,
    // priority and payload; the first copy stays cached
    assert!(!harness.is_track_malformed());
    assert_eq!(harness.cache().occupancy().objects, 2);
}
