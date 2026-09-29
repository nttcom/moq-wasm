use std::time::Duration;

use bytes::Bytes;

use moqt::DataStreamResetCode;

use crate::modules::relay::tests::harness::{FetchSent, RelayHarness, fixtures::location};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closed_groups_are_delivered_before_a_later_open_group_closes() {
    // Arrange: group 0 closed, group 1 still open, group 2 closed
    let harness = RelayHarness::new();
    let group0 = harness.open_upstream_stream();
    group0.header(0);
    group0.object(0);
    group0.object(1);
    group0.fin();
    harness.wait_group_closed(0).await;
    let group1 = harness.open_upstream_stream();
    group1.header(1);
    group1.object(0);
    group1.object(1);
    harness.wait_largest_location(location(1, 1)).await;
    let group2 = harness.open_upstream_stream();
    group2.header(2);
    group2.object(0);
    group2.fin();
    harness.wait_group_closed(2).await;

    // Act
    let mut fetch = harness.start_fetch(location(0, 0), location(2, 0));

    // Assert: everything known so far is sent while group 1 is still open
    fetch
        .expect_objects(&[(0, 0), (0, 1), (1, 0), (1, 1)])
        .await;
    fetch
        .assert_nothing_sent_within(Duration::from_millis(200))
        .await;
    group1.object(2);
    group1.fin();
    fetch.expect_objects(&[(1, 2), (2, 0)]).await;
    assert!(matches!(fetch.expect_end().await, FetchSent::Closed));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_track_resets_the_fetch_stream() {
    // Arrange: the fetch waits on the open group's tail
    let harness = RelayHarness::new();
    let first_stream = harness.open_upstream_stream();
    first_stream.header(0);
    first_stream.object(0);
    harness.wait_largest_location(location(0, 0)).await;
    let mut fetch = harness.start_fetch(location(0, 0), location(0, 3));
    fetch.expect_objects(&[(0, 0)]).await;

    // Act: a second stream re-delivers object 0 with a different payload
    let second_stream = harness.open_upstream_stream();
    second_stream.header(0);
    second_stream.object_with_payload(Bytes::from_static(b"conflicting"));

    // Assert
    assert!(matches!(
        fetch.expect_end().await,
        FetchSent::Reset(code) if code == DataStreamResetCode::MalformedTrack
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn aborted_upstream_stream_resets_the_fetch_stream() {
    // Arrange: the fetch waits on the open group's tail
    let harness = RelayHarness::new();
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(0);
    upstream_stream.object(0);
    harness.wait_largest_location(location(0, 0)).await;
    let mut fetch = harness.start_fetch(location(0, 0), location(0, 3));
    fetch.expect_objects(&[(0, 0)]).await;

    // Act
    upstream_stream.reset();

    // Assert
    assert!(matches!(
        fetch.expect_end().await,
        FetchSent::Reset(code) if code == DataStreamResetCode::InternalError
    ));
}
