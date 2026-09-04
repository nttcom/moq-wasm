use super::harness::{
    OBJECT_COUNT, RelayHarness, assert_full_ordered_delivery, receive_objects_until_close,
    resolve_downstream_object_ids,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn burst_publish_with_immediate_fin_delivers_all_objects() {
    for _ in 0..100 {
        let harness = RelayHarness::new();
        let mut egress = harness.start_egress(None).await;

        let upstream_stream = harness.open_upstream_stream().await;
        upstream_stream.header(0);
        for index in 0..OBJECT_COUNT {
            upstream_stream.object(index);
        }
        upstream_stream.fin();

        let objects = receive_objects_until_close(&mut egress).await;
        assert_full_ordered_delivery(&objects);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn egress_start_racing_ingest_burst_delivers_all_objects() {
    for _ in 0..100 {
        let harness = RelayHarness::new();

        let upstream_stream = harness.open_upstream_stream().await;
        upstream_stream.header(0);
        for index in 0..OBJECT_COUNT {
            upstream_stream.object(index);
        }
        upstream_stream.fin();

        let mut egress = harness.start_egress(None).await;

        let objects = receive_objects_until_close(&mut egress).await;
        assert_full_ordered_delivery(&objects);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn egress_started_with_pre_subscribe_snapshot_delivers_head_objects_cached_mid_burst() {
    const IN_FLIGHT_BEFORE_EGRESS_START: usize = 10;

    let harness = RelayHarness::new();
    let snapshot_before_subscribe = None;

    let upstream_stream = harness.open_upstream_stream().await;
    upstream_stream.header(0);
    for index in 0..IN_FLIGHT_BEFORE_EGRESS_START {
        upstream_stream.object(index);
    }
    harness
        .wait_largest_location(moqt::Location {
            group_id: 0,
            object_id: (IN_FLIGHT_BEFORE_EGRESS_START - 1) as u64,
        })
        .await;

    let mut egress = harness.start_egress(snapshot_before_subscribe).await;

    for index in IN_FLIGHT_BEFORE_EGRESS_START..OBJECT_COUNT {
        upstream_stream.object(index);
    }
    upstream_stream.fin();

    let objects = receive_objects_until_close(&mut egress).await;
    assert_full_ordered_delivery(&objects);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn egress_started_mid_subgroup_delivers_absolute_object_ids() {
    const LARGEST_OBJECT_ID_AT_SUBSCRIBE: usize = 9;

    let harness = RelayHarness::new();
    let upstream_stream = harness.open_upstream_stream().await;
    upstream_stream.header(0);
    for index in 0..=LARGEST_OBJECT_ID_AT_SUBSCRIBE {
        upstream_stream.object(index);
    }
    let largest = harness
        .wait_largest_location(moqt::Location {
            group_id: 0,
            object_id: LARGEST_OBJECT_ID_AT_SUBSCRIBE as u64,
        })
        .await;

    let mut egress = harness.start_egress(Some(largest)).await;
    for index in LARGEST_OBJECT_ID_AT_SUBSCRIBE + 1..OBJECT_COUNT {
        upstream_stream.object(index);
    }
    upstream_stream.fin();

    let objects = receive_objects_until_close(&mut egress).await;
    let expected: Vec<u64> =
        (LARGEST_OBJECT_ID_AT_SUBSCRIBE as u64 + 1..OBJECT_COUNT as u64).collect();
    assert_eq!(resolve_downstream_object_ids(&objects), expected);
}
