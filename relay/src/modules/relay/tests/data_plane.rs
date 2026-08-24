use super::harness::{
    DataPlaneHarness, OBJECT_COUNT, assert_full_ordered_delivery, receive_objects_until_close,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn burst_publish_with_immediate_fin_delivers_all_objects() {
    for _ in 0..100 {
        let harness = DataPlaneHarness::new();
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
        let harness = DataPlaneHarness::new();

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
#[ignore = "known bug (cascading-relay E2E flake): objects ingested between upstream ingress start and largest-location resolution shift the delivery start, losing the head of the group; un-ignore with the fix"]
async fn largest_location_resolved_mid_burst_must_not_skip_head_objects() {
    const IN_FLIGHT_BEFORE_RESOLVE: usize = 10;

    let harness = DataPlaneHarness::new();

    let upstream_stream = harness.open_upstream_stream().await;
    upstream_stream.header(0);
    for index in 0..IN_FLIGHT_BEFORE_RESOLVE {
        upstream_stream.object(index);
    }
    let largest_at_subscribe_ok = harness
        .wait_largest_location(moqt::Location {
            group_id: 0,
            object_id: (IN_FLIGHT_BEFORE_RESOLVE - 1) as u64,
        })
        .await;

    let mut egress = harness.start_egress(Some(largest_at_subscribe_ok)).await;

    for index in IN_FLIGHT_BEFORE_RESOLVE..OBJECT_COUNT {
        upstream_stream.object(index);
    }
    upstream_stream.fin();

    let objects = receive_objects_until_close(&mut egress).await;
    assert_full_ordered_delivery(&objects);
}
