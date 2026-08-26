//! Integration tests for draft-14 §2.5 Malformed Tracks, condition 8:
//! "The same Object is received more than once with different Payload or
//! other immutable properties."

use bytes::Bytes;

use crate::modules::{
    core::data_object::DataObject,
    enums::PublishDoneStatusCode,
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
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;

    // A second stream re-delivers object 0 of the same (group, subgroup)
    // with a different payload — §2.5 condition 8.
    let first_stream = harness.open_upstream_stream().await;
    first_stream.header(0);
    first_stream.object(0);
    let second_stream = harness.open_upstream_stream().await;
    second_stream.header(0);
    second_stream.object_with_payload(0, Bytes::from_static(b"conflicting"));

    // The relay must terminate the downstream subscription with PUBLISH_DONE
    // carrying MALFORMED_TRACK, keeping the session itself alive.
    let publish_done = egress.expect_publish_done().await;
    assert_eq!(
        publish_done.status_code,
        PublishDoneStatusCode::MalformedTrack as u64
    );
    assert_eq!(publish_done.request_id, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_duplicate_from_second_stream_is_not_malformed() {
    // Cascading topologies legitimately deliver the same object via several
    // paths; a byte-identical duplicate must be deduped, not flagged.
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;

    let first_stream = harness.open_upstream_stream().await;
    first_stream.header(0);
    first_stream.object(0);
    let second_stream = harness.open_upstream_stream().await;
    second_stream.header(0);
    second_stream.object(0);
    // The two streams share one subgroup entry, so close it only after the
    // last object is cached: a FIN racing ahead of a late append (processed by
    // the other stream's reader task) would cut delivery short.
    first_stream.object(1);
    harness
        .wait_largest_location(moqt::Location {
            group_id: 0,
            object_id: 1,
        })
        .await;
    first_stream.fin();
    second_stream.fin();

    let objects = receive_objects_until_close(&mut egress).await;
    assert_eq!(
        payloads_of(&objects),
        vec![ordered_payload(0), ordered_payload(1)]
    );
    egress.assert_no_publish_done();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscription_started_after_detection_is_terminated_immediately() {
    let harness = RelayHarness::new();

    // Latch the track first, without any downstream subscriber attached.
    let first_stream = harness.open_upstream_stream().await;
    first_stream.header(0);
    first_stream.object(0);
    let second_stream = harness.open_upstream_stream().await;
    second_stream.header(0);
    second_stream.object_with_payload(0, Bytes::from_static(b"conflicting"));
    harness.wait_track_malformed().await;

    // A runner started on the quarantined track terminates right away with
    // Stream Count 0: it never opened a data stream.
    let mut egress = harness.start_egress(None).await;
    let publish_done = egress.expect_publish_done().await;
    assert_eq!(
        publish_done.status_code,
        PublishDoneStatusCode::MalformedTrack as u64
    );
    assert_eq!(publish_done.stream_count, 0);
}
