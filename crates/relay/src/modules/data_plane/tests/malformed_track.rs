use moqt::wire::publish_done_status_code;

use crate::modules::{
    data_plane::tests::harness::{
        PUBLISHER_SESSION_ID, RelayHarness, fixtures::location, ordered_payload, payloads_of,
        receive_objects_until_close,
    },
    session_event::EventKind,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicate_object_with_different_payload_terminates_subscription() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;

    // Act
    let _streams = harness.ingest_conflicting_duplicate();

    // Assert
    let publish_done = egress.expect_publish_done().await;
    assert_eq!(
        publish_done.status_code,
        publish_done_status_code::MALFORMED_TRACK
    );
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
    assert_eq!(
        publish_done.status_code,
        publish_done_status_code::MALFORMED_TRACK
    );
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
