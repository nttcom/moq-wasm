use bytes::Bytes;

use moqt::wire::publish_done_status_code;

use crate::modules::{
    core::data_object::DataObject,
    relay::tests::harness::{RelayHarness, ordered_payload, receive_objects_until_close},
};

fn payloads_of(objects: &[DataObject]) -> Vec<Bytes> {
    objects
        .iter()
        .filter_map(|object| match object {
            DataObject::SubgroupObject(field) => match &field.subgroup_object {
                moqt::SubgroupObject::Payload { data, .. } => Some(data.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicate_object_with_different_payload_terminates_subscription() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;

    // Act: a second stream re-delivers object 0 with a different payload
    let first_stream = harness.open_upstream_stream().await;
    first_stream.header(0);
    first_stream.object(0);
    let second_stream = harness.open_upstream_stream().await;
    second_stream.header(0);
    second_stream.object_with_payload(0, Bytes::from_static(b"conflicting"));

    // Assert: the subscription ends with PUBLISH_DONE(MALFORMED_TRACK)
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
    let first_stream = harness.open_upstream_stream().await;
    first_stream.header(0);
    first_stream.object(0);
    let second_stream = harness.open_upstream_stream().await;
    second_stream.header(0);
    second_stream.object(0);
    first_stream.object(1);
    harness
        .wait_largest_location(moqt::Location {
            group_id: 0,
            object_id: 1,
        })
        .await;
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

    let first_stream = harness.open_upstream_stream().await;
    first_stream.header(0);
    first_stream.object(0);
    let second_stream = harness.open_upstream_stream().await;
    second_stream.header(0);
    second_stream.object_with_payload(0, Bytes::from_static(b"conflicting"));
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
