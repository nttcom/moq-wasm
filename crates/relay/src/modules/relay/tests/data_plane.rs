use std::time::Duration;

use crate::modules::core::data_object::DataObject;

use super::harness::{
    OBJECT_COUNT, RelayHarness, Sent, assert_full_ordered_delivery,
    fixtures::{cached_object::FIXTURE_PRIORITY, location},
    ordered_payload, payloads_of, receive_objects_until_close, receive_objects_until_end,
    resolve_downstream_object_ids,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn burst_publish_with_immediate_fin_delivers_all_objects() {
    for _ in 0..100 {
        // Arrange
        let harness = RelayHarness::new();
        let mut egress = harness.start_egress(None).await;

        // Act
        let upstream_stream = harness.open_upstream_stream();
        upstream_stream.header(0);
        for index in 0..OBJECT_COUNT {
            upstream_stream.object(index);
        }
        upstream_stream.fin();

        // Assert
        let objects = receive_objects_until_close(&mut egress).await;
        assert_full_ordered_delivery(&objects);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn egress_start_racing_ingest_burst_delivers_all_objects() {
    for _ in 0..100 {
        // Arrange
        let harness = RelayHarness::new();
        let upstream_stream = harness.open_upstream_stream();
        upstream_stream.header(0);
        for index in 0..OBJECT_COUNT {
            upstream_stream.object(index);
        }
        upstream_stream.fin();

        // Act
        let mut egress = harness.start_egress(None).await;

        // Assert
        let objects = receive_objects_until_close(&mut egress).await;
        assert_full_ordered_delivery(&objects);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn next_group_start_delivers_a_first_group_with_nonzero_id_cached_before_egress_started() {
    const FIRST_GROUP_ID: u64 = 1_757_300_000_000_000;

    // Arrange
    let harness = RelayHarness::new();
    let no_content_before_subscribe = None;
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(FIRST_GROUP_ID);
    upstream_stream.object(0);
    upstream_stream.fin();
    harness
        .wait_largest_location(location(FIRST_GROUP_ID, 0))
        .await;

    // Act
    let mut egress = harness
        .start_egress_with_filter(
            moqt::FilterType::NextGroupStart,
            no_content_before_subscribe,
        )
        .await;

    // Assert
    let objects = receive_objects_until_close(&mut egress).await;
    assert!(
        matches!(
            objects.first(),
            Some(DataObject::SubgroupHeader(header)) if header.group_id == FIRST_GROUP_ID
        ),
        "downstream stream should start with the first group's subgroup header"
    );
    assert_eq!(resolve_downstream_object_ids(&objects), vec![0]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn egress_started_with_pre_subscribe_snapshot_delivers_head_objects_cached_mid_burst() {
    const IN_FLIGHT_BEFORE_EGRESS_START: usize = 10;

    // Arrange
    let harness = RelayHarness::new();
    let snapshot_before_subscribe = None;
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(0);
    for index in 0..IN_FLIGHT_BEFORE_EGRESS_START {
        upstream_stream.object(index);
    }
    harness
        .wait_largest_location(location(0, (IN_FLIGHT_BEFORE_EGRESS_START - 1) as u64))
        .await;

    // Act
    let mut egress = harness.start_egress(snapshot_before_subscribe).await;
    for index in IN_FLIGHT_BEFORE_EGRESS_START..OBJECT_COUNT {
        upstream_stream.object(index);
    }
    upstream_stream.fin();

    // Assert
    let objects = receive_objects_until_close(&mut egress).await;
    assert_full_ordered_delivery(&objects);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn egress_started_mid_subgroup_delivers_absolute_object_ids() {
    const LARGEST_OBJECT_ID_AT_SUBSCRIBE: usize = 9;

    // Arrange
    let harness = RelayHarness::new();
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(0);
    for index in 0..=LARGEST_OBJECT_ID_AT_SUBSCRIBE {
        upstream_stream.object(index);
    }
    let largest = harness
        .wait_largest_location(location(0, LARGEST_OBJECT_ID_AT_SUBSCRIBE as u64))
        .await;

    // Act
    let mut egress = harness.start_egress(Some(largest)).await;
    for index in LARGEST_OBJECT_ID_AT_SUBSCRIBE + 1..OBJECT_COUNT {
        upstream_stream.object(index);
    }
    upstream_stream.fin();

    // Assert
    let objects = receive_objects_until_close(&mut egress).await;
    let expected: Vec<u64> =
        (LARGEST_OBJECT_ID_AT_SUBSCRIBE as u64 + 1..OBJECT_COUNT as u64).collect();
    assert_eq!(resolve_downstream_object_ids(&objects), expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subgroup_closed_without_objects_opens_no_downstream_stream() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;

    // Act
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(0);
    upstream_stream.fin();

    // Assert
    egress
        .assert_nothing_sent_within(Duration::from_millis(200))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn downstream_header_is_regenerated_from_the_cached_objects() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;

    // Act
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(0);
    upstream_stream.object(0);
    upstream_stream.fin();

    // Assert
    let objects = receive_objects_until_close(&mut egress).await;
    let Some(DataObject::SubgroupHeader(header)) = objects.first() else {
        panic!("downstream stream should start with a subgroup header");
    };
    assert_eq!(header.group_id, 0);
    assert_eq!(header.subgroup_id, moqt::SubgroupId::Value(0));
    assert_eq!(header.publisher_priority, FIXTURE_PRIORITY);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn later_group_stream_opens_with_a_lower_transport_priority() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    // Act
    let first_group = harness.open_upstream_stream();
    first_group.header(0);
    first_group.object(0);
    let first_priority = egress.expect_stream_priority().await;
    let second_group = harness.open_upstream_stream();
    second_group.header(1);
    second_group.object(0);
    let second_priority = egress.expect_stream_priority().await;
    // Assert
    assert_eq!(
        (
            first_priority.group_sequence,
            second_priority.group_sequence
        ),
        (0, 1)
    );
    assert_eq!(first_priority.publisher_priority, FIXTURE_PRIORITY);
    assert!(
        first_priority.transport_priority() > second_priority.transport_priority(),
        "the earlier group must be transmitted first under Ascending group order"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upstream_reset_is_relayed_as_a_downstream_reset() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;

    // Act
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(0);
    for index in 0..3 {
        upstream_stream.object(index);
    }
    upstream_stream.reset();

    // Assert
    let (objects, end) = receive_objects_until_end(&mut egress).await;
    assert_eq!(
        resolve_downstream_object_ids(&objects),
        vec![0, 1, 2],
        "objects received before the reset are still forwarded"
    );
    assert!(
        matches!(end, Sent::Reset(0)),
        "a partial subgroup must be reset downstream with INTERNAL_ERROR, got {end:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscription_with_forward_off_opens_no_stream_for_a_new_group() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    egress.set_forward(false);

    // Act
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(0);
    upstream_stream.object(0);
    upstream_stream.fin();

    // Assert
    egress
        .assert_nothing_sent_within(Duration::from_millis(200))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscription_with_forward_back_on_resumes_from_the_next_group() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    egress.set_forward(false);
    let skipped_group = harness.open_upstream_stream();
    skipped_group.header(0);
    skipped_group.object(0);
    egress
        .assert_nothing_sent_within(Duration::from_millis(200))
        .await;

    // Act
    egress.set_forward(true);
    skipped_group.object(1);
    skipped_group.fin();
    let next_group = harness.open_upstream_stream();
    next_group.header(1);
    next_group.object(0);
    next_group.fin();

    // Assert
    let objects = receive_objects_until_close(&mut egress).await;
    let Some(DataObject::SubgroupHeader(header)) = objects.first() else {
        panic!("downstream stream should start with a subgroup header");
    };
    assert_eq!(header.group_id, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscriber_joining_after_an_upstream_reset_receives_the_reopened_subgroup() {
    // Arrange
    let harness = RelayHarness::new();
    let mut reset_stream = harness.open_upstream_stream();
    reset_stream.header(0);
    for index in 0..3 {
        reset_stream.object(index);
    }
    reset_stream.reset();
    reset_stream.wait_reader_end().await;
    let mut egress = harness.start_egress(Some(location(0, 2))).await;
    // Arrange: give the egress stream task time to find the subgroup aborted
    egress
        .assert_nothing_sent_within(Duration::from_millis(50))
        .await;

    // Act
    let reopened_stream = harness.open_upstream_stream();
    reopened_stream.header(0);
    reopened_stream.object_with_delta(3, 3);
    reopened_stream.object(4);
    reopened_stream.object(5);
    reopened_stream.fin();

    // Assert
    let objects = receive_objects_until_close(&mut egress).await;
    assert_eq!(resolve_downstream_object_ids(&objects), vec![3, 4, 5]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upstream_reopen_after_a_reset_is_relayed_on_a_new_downstream_stream() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let reset_stream = harness.open_upstream_stream();
    reset_stream.header(0);
    for index in 0..3 {
        reset_stream.object(index);
    }
    reset_stream.reset();
    let (reset_objects, reset_end) = receive_objects_until_end(&mut egress).await;

    // Act: the reopened stream skips object 3
    let reopened_stream = harness.open_upstream_stream();
    reopened_stream.header(0);
    reopened_stream.object_with_delta(4, 4);
    reopened_stream.object(5);
    reopened_stream.fin();

    // Assert
    let reopened_objects = receive_objects_until_close(&mut egress).await;
    assert_eq!(resolve_downstream_object_ids(&reset_objects), vec![0, 1, 2]);
    assert!(matches!(reset_end, Sent::Reset(0)), "got {reset_end:?}");
    assert!(matches!(
        reopened_objects.first(),
        Some(DataObject::SubgroupHeader(_))
    ));
    assert_eq!(resolve_downstream_object_ids(&reopened_objects), vec![4, 5]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upstream_reopen_racing_the_reset_relays_each_object_once_on_two_streams() {
    for _ in 0..100 {
        // Arrange
        let harness = RelayHarness::new();
        let mut egress = harness.start_egress(None).await;
        let mut reset_stream = harness.open_upstream_stream();
        reset_stream.header(0);
        for index in 0..3 {
            reset_stream.object(index);
        }

        // Act
        reset_stream.reset();
        reset_stream.wait_reader_end().await;
        let reopened_stream = harness.open_upstream_stream();
        reopened_stream.header(0);
        reopened_stream.object_with_delta(4, 4);
        reopened_stream.object(5);
        reopened_stream.fin();

        // Assert: the two downstream streams may interleave
        let (mut objects, first_end) = receive_objects_until_end(&mut egress).await;
        let (second_objects, second_end) = receive_objects_until_end(&mut egress).await;
        objects.extend(second_objects);
        let header_count = objects
            .iter()
            .filter(|object| matches!(object, DataObject::SubgroupHeader(_)))
            .count();
        let mut payloads = payloads_of(&objects);
        payloads.sort();
        assert_eq!(header_count, 2);
        assert_eq!(
            payloads,
            [0, 1, 2, 4, 5].map(ordered_payload).to_vec(),
            "each object is relayed exactly once"
        );
        assert!(
            matches!(
                (&first_end, &second_end),
                (Sent::Reset(0), Sent::Closed) | (Sent::Closed, Sent::Reset(0))
            ),
            "the reset stream is reset and the reopened one finished, got {first_end:?} and {second_end:?}"
        );
    }
}
