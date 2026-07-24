//! Live forwarding pipeline scenarios: ingress `StreamReader` ->
//! `TrackCache` -> `EgressRunner` (scheduler + group sender) -> downstream
//! `DataSender`.
//!
//! These target the cascading-relay E2E flake "stream ended before receiving
//! 50 objects": a publisher that answers SUBSCRIBE with a burst of objects on
//! one subgroup stream and closes it immediately (FIN) races the relay's
//! subscribe handling, and the downstream subscriber observes a truncated
//! stream. The tests pin the pipeline behavior at each stage of that race so
//! a failure names the stage instead of an opaque E2E timeout.

use bytes::Bytes;

use super::harness::{
    OBJECT_COUNT, PipelineHarness, assert_full_ordered_delivery, collect_until_closed,
    header_group_on_stream, ordered_payload, payloads_on_stream, stream_closed,
};

/// Baseline: egress is running (largest resolved as None before any object
/// existed) when the publisher bursts one subgroup of 50 objects and FINs
/// immediately. Every object must reach the downstream before its stream
/// closes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn burst_publish_with_immediate_fin_delivers_all_objects() {
    for round in 0..100 {
        let harness = PipelineHarness::new();
        let mut egress = harness.start_egress(None).await;

        let feed = harness.open_upstream_stream().await;
        feed.header(0);
        for index in 0..OBJECT_COUNT {
            feed.object(index);
        }
        feed.fin();

        let events = collect_until_closed(&mut egress, 1).await;
        let payloads = payloads_on_stream(&events, 0);
        assert_eq!(
            payloads.len(),
            OBJECT_COUNT,
            "round {round}: downstream stream ended before receiving {OBJECT_COUNT} objects"
        );
        assert_full_ordered_delivery(&events);
    }
}

/// The egress starts while the burst is already in flight (subscribe-time
/// largest was resolved as None before the burst reached the relay). The
/// scheduler must catch up from the cache regardless of whether the
/// StreamOpened notification fired before it subscribed to track events.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn egress_start_racing_ingest_burst_delivers_all_objects() {
    for round in 0..100 {
        let harness = PipelineHarness::new();

        let feed = harness.open_upstream_stream().await;
        feed.header(0);
        for index in 0..OBJECT_COUNT {
            feed.object(index);
        }
        feed.fin();

        // No await between feeding and starting the egress: the ingest and
        // the egress startup interleave differently on every round.
        let mut egress = harness.start_egress(None).await;

        let events = collect_until_closed(&mut egress, 1).await;
        let payloads = payloads_on_stream(&events, 0);
        assert_eq!(
            payloads.len(),
            OBJECT_COUNT,
            "round {round}: downstream stream ended before receiving {OBJECT_COUNT} objects"
        );
        assert_full_ordered_delivery(&events);
    }
}

/// The subscriber arrives after the whole subgroup was ingested and closed
/// (fastest possible FIN). A closed subgroup must still be drained in full;
/// closing must never truncate objects already in the cache.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closed_subgroup_is_drained_in_full() {
    let harness = PipelineHarness::new();
    let mut track_events = harness
        .notify_map
        .get_or_create(&harness.track_key)
        .subscribe();

    let feed = harness.open_upstream_stream().await;
    feed.header(0);
    for index in 0..OBJECT_COUNT {
        feed.object(index);
    }
    feed.fin();
    harness.wait_end_of_group(&mut track_events).await;

    let mut egress = harness.start_egress(None).await;

    let events = collect_until_closed(&mut egress, 1).await;
    assert_full_ordered_delivery(&events);
}

/// Reproduces the cascading-relay E2E flake "stream ended before receiving 50
/// objects" (ordered objects scenario, e.g. CI runs 29805931921, 29481445955,
/// 29477073181).
///
/// Production ordering in `sequences::subscribe`: the upstream ingress starts
/// on SUBSCRIBE_OK, and only afterwards `accept_downstream_subscription`
/// resolves the subscribe-time Largest Location from the live cache
/// (`resolve_subscribe_largest`). A publisher that answers SUBSCRIBE by
/// bursting objects immediately (the E2E publisher sends all 50 within ~1ms)
/// lands part of the burst in the cache inside that window. The resolved
/// largest then points into the burst, the LargestObject filter starts
/// delivery just after it, and the downstream subscriber - whose SUBSCRIBE is
/// what triggered this publish, and which observed no content at subscribe
/// time - permanently loses the head of the group.
///
/// The relay must not let objects that arrived after the triggering SUBSCRIBE
/// shift that subscriber's delivery start.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "known bug: in-flight burst shifts the resolved largest location and the head of the group is lost; un-ignore with the fix"]
async fn largest_location_resolved_mid_burst_must_not_skip_head_objects() {
    const IN_FLIGHT_BEFORE_RESOLVE: usize = 10;

    let harness = PipelineHarness::new();

    // The burst races ahead: the first objects reach the cache before the
    // subscribe sequence resolves the largest location.
    let feed = harness.open_upstream_stream().await;
    feed.header(0);
    for index in 0..IN_FLIGHT_BEFORE_RESOLVE {
        feed.object(index);
    }
    let largest = harness
        .wait_largest_location(moqt::Location {
            group_id: 0,
            object_id: (IN_FLIGHT_BEFORE_RESOLVE - 1) as u64,
        })
        .await;

    // What `accept_downstream_subscription` does now: largest from the live
    // cache -> EgressStartRequest -> SUBSCRIBE_OK.
    let mut egress = harness.start_egress(Some(largest)).await;

    for index in IN_FLIGHT_BEFORE_RESOLVE..OBJECT_COUNT {
        feed.object(index);
    }
    feed.fin();

    let events = collect_until_closed(&mut egress, 1).await;
    assert_full_ordered_delivery(&events);
}

/// Multi-group variant closer to the media E2E shape: several subgroup
/// streams in sequence, each closed with FIN right after its last object.
/// Every group must arrive complete on its own downstream stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sequential_groups_with_fast_fin_each_arrive_complete() {
    const GROUPS: u64 = 10;
    const OBJECTS_PER_GROUP: usize = 5;

    let harness = PipelineHarness::new();
    let mut egress = harness.start_egress(None).await;

    for group_id in 0..GROUPS {
        let feed = harness.open_upstream_stream().await;
        feed.header(group_id);
        for index in 0..OBJECTS_PER_GROUP {
            feed.object(index);
        }
        feed.fin();
    }

    let events = collect_until_closed(&mut egress, GROUPS as usize).await;

    let mut delivered_groups = Vec::new();
    for stream in 0..GROUPS as usize {
        let group_id = header_group_on_stream(&events, stream)
            .unwrap_or_else(|| panic!("stream {stream} should carry a subgroup header"));
        let payloads = payloads_on_stream(&events, stream);
        let expected: Vec<Bytes> = (0..OBJECTS_PER_GROUP).map(ordered_payload).collect();
        assert_eq!(
            payloads, expected,
            "group {group_id} on stream {stream} must arrive complete and in order"
        );
        assert!(
            stream_closed(&events, stream),
            "stream {stream} should be closed after its last object"
        );
        delivered_groups.push(group_id);
    }
    delivered_groups.sort_unstable();
    assert_eq!(
        delivered_groups,
        (0..GROUPS).collect::<Vec<_>>(),
        "every group must be delivered exactly once"
    );
}
