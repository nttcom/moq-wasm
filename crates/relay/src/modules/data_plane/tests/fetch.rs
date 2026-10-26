use moqt::object::{DataStreamResetCode, ForwardingPreference};
use std::time::Duration;

use bytes::Bytes;

use crate::modules::test_support::relay_harness::{
    FetchSent, RelayHarness,
    fixtures::{
        cached_object::{datagram_object, fetched_copy_in_subgroup},
        location,
    },
};

/// The forwarding preference a FETCH from the cache gives a draft-14 FETCH
/// copy at {1, 2} in subgroup 2.
async fn served_forwarding_of_a_fetched_copy(harness: &RelayHarness) -> ForwardingPreference {
    harness
        .cache()
        .insert(fetched_copy_in_subgroup(1, 2, 2))
        .unwrap();
    harness
        .cache()
        .insert_fetch_known_range(location(1, 0), location(2, 0));
    let mut fetch = harness.start_fetch(location(1, 0), location(2, 0));
    fetch.expect_object().await.forwarding
}

#[tokio::test]
async fn a_draft14_fetch_copy_of_a_datagram_is_served_as_a_datagram_on_a_datagram_track() {
    // Arrange: live ingest has delivered the track's objects as datagrams
    let harness = RelayHarness::new();
    let upstream = harness.start_track_ingest();
    upstream.datagram(datagram_object(0, 0).object);
    harness.wait_largest_location(location(0, 0)).await;

    // Act
    let forwarding = served_forwarding_of_a_fetched_copy(&harness).await;

    // Assert: draft-18 §11.4.4 marks it, so it matches the live datagram a
    // draft-18 subscriber may hold for that location
    assert_eq!(forwarding, ForwardingPreference::Datagram);
}

#[tokio::test]
async fn a_draft14_fetch_copy_is_served_as_received_on_a_track_without_live_datagrams() {
    // Arrange
    let harness = RelayHarness::new();

    // Act
    let forwarding = served_forwarding_of_a_fetched_copy(&harness).await;

    // Assert
    assert_eq!(
        forwarding,
        ForwardingPreference::Subgroup { subgroup_id: 2 }
    );
}

#[tokio::test]
async fn a_draft14_fetch_copy_is_served_as_received_once_live_objects_come_on_streams_again() {
    // Arrange: a later publisher of the track sends on subgroup streams
    let harness = RelayHarness::new();
    let earlier = harness.start_track_ingest();
    earlier.datagram(datagram_object(0, 0).object);
    harness.wait_largest_location(location(0, 0)).await;
    let later = harness.open_upstream_stream();
    later.header(3);
    later.object(0);
    harness.wait_largest_location(location(3, 0)).await;

    // Act
    let forwarding = served_forwarding_of_a_fetched_copy(&harness).await;

    // Assert
    assert_eq!(
        forwarding,
        ForwardingPreference::Subgroup { subgroup_id: 2 }
    );
}

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

/// Groups 0 and 2 closed, group 1 never received.
async fn harness_missing_group_1() -> RelayHarness {
    let harness = RelayHarness::new();
    for (group_id, object_count) in [(0, 2), (2, 1)] {
        let stream = harness.open_upstream_stream();
        stream.header(group_id);
        for index in 0..object_count {
            stream.object(index);
        }
        stream.fin();
        harness.wait_group_closed(group_id).await;
    }
    harness
}

fn sent_steps(sent: Vec<FetchSent>) -> Vec<String> {
    sent.into_iter()
        .map(|step| match step {
            FetchSent::Object(object) => {
                format!("{}/{}", object.location.group_id, object.location.object_id)
            }
            FetchSent::UnknownRange(last) => {
                format!("unknown..{}/{}", last.group_id, last.object_id)
            }
            FetchSent::Closed => "fin".to_string(),
            FetchSent::Reset(code) => format!("reset {code:?}"),
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fetch_reporting_unknown_marks_what_the_cache_lacks_and_ends_with_fin() {
    // Arrange
    let harness = harness_missing_group_1().await;

    // Act
    let fetch = harness.start_fetch_reporting_unknown(
        location(0, 0),
        location(2, 0),
        moqt::fetch::UnknownRange::Marked,
    );

    // Assert: draft-18 §10.12, the known objects around the unknown group are served
    assert_eq!(
        sent_steps(fetch.finished().await),
        vec![
            "0/0".to_string(),
            "0/1".to_string(),
            format!("unknown..1/{}", u64::MAX),
            "2/0".to_string(),
            "fin".to_string(),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fetch_reporting_unknown_stops_once_the_sender_has_reset() {
    // Arrange
    let harness = harness_missing_group_1().await;

    // Act: the sender cannot express the range, as on draft-14
    let fetch = harness.start_fetch_reporting_unknown(
        location(0, 0),
        location(2, 0),
        moqt::fetch::UnknownRange::Reset,
    );

    // Assert: draft-18 §10.12, the reset ends the response
    assert_eq!(
        sent_steps(fetch.finished().await),
        vec![
            "0/0".to_string(),
            "0/1".to_string(),
            format!("unknown..1/{}", u64::MAX),
        ]
    );
}
