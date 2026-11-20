//! An object at Object ID u64::MAX, the last of its group: a draft-18
//! publisher reaches it through Object ID deltas.

use std::time::Duration;

use moqt::object::SubgroupPart;

use crate::modules::test_support::relay_harness::{
    EgressRunnerHandle, FetchSent, RelayHarness, Sent,
    fixtures::{
        cached_object::{datagram_object, insert_closed_group, stream_object},
        data_object::make_header_with,
        location,
    },
};

const NOTHING_MORE_WINDOW: Duration = Duration::from_millis(200);

async fn next_steps(egress: &mut EgressRunnerHandle, count: usize) -> Vec<String> {
    let mut steps = Vec::new();
    while steps.len() < count {
        steps.push(match egress.expect_sent().await {
            Sent::Object(SubgroupPart::Header(start)) => format!("header {}", start.group_id),
            Sent::Object(SubgroupPart::Object(object)) | Sent::Datagram(object) => {
                format!("{}/{}", object.location.group_id, object.location.object_id)
            }
            Sent::Closed => "fin".to_string(),
            Sent::Reset(code) => format!("reset {code:?}"),
            other => panic!("unexpected {other:?}"),
        });
    }
    steps
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subgroup_ending_at_the_largest_object_id_sends_it_once_then_closes() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let upstream = harness.open_upstream_stream();

    // Act
    upstream.send(make_header_with(0, 0, false));
    upstream.send(SubgroupPart::Object(stream_object(0, u64::MAX - 1).object));
    upstream.send(SubgroupPart::Object(stream_object(0, u64::MAX).object));
    upstream.fin();

    // Assert
    assert_eq!(
        next_steps(&mut egress, 4).await,
        vec![
            "header 0".to_string(),
            format!("0/{}", u64::MAX - 1),
            format!("0/{}", u64::MAX),
            "fin".to_string(),
        ]
    );
    egress.assert_nothing_sent_within(NOTHING_MORE_WINDOW).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_datagram_at_the_largest_object_id_is_sent_once() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let upstream = harness.start_track_ingest();

    // Act
    upstream.datagram(datagram_object(0, u64::MAX).object);
    upstream.datagram(datagram_object(1, 0).object);

    // Assert: the datagram sends of the two groups run apart, so their order is not kept
    let mut steps = next_steps(&mut egress, 2).await;
    steps.sort();
    assert_eq!(steps, vec![format!("0/{}", u64::MAX), "1/0".to_string()]);
    egress.assert_nothing_sent_within(NOTHING_MORE_WINDOW).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fetch_of_a_group_ending_at_the_largest_object_id_sends_it_once() {
    // Arrange
    let harness = RelayHarness::new();
    insert_closed_group(harness.cache(), 0, &[u64::MAX]);

    // Act
    let mut fetch = harness.start_fetch(location(0, 0), location(0, 0));

    // Assert
    fetch.expect_objects(&[(0, u64::MAX)]).await;
    assert!(matches!(fetch.expect_end().await, FetchSent::Closed));
}
