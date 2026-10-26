use std::time::Duration;

use moqt::object::{ForwardingPreference, SubgroupStart};

use crate::modules::{
    domain::single_preference::CONVERTED_DATAGRAM_SUBGROUPS,
    session::data_object::DataObject,
    test_support::relay_harness::{
        EgressRunnerHandle, FetchSent, RelayHarness, Sent, UpstreamTrack,
        fixtures::{
            cached_object::datagram_object,
            data_object::{make_header_with, make_object_in_subgroup, subgroup_start},
            location,
        },
    },
};

const NO_PUBLISH_DONE_WINDOW: Duration = Duration::from_millis(200);

/// `(datagram, group, object)` of each object sent downstream, sorted: the
/// datagram and stream senders run apart, so their order is not kept.
async fn sent_objects(egress: &mut EgressRunnerHandle, count: usize) -> Vec<(bool, u64, u64)> {
    let mut sent = Vec::new();
    while sent.len() < count {
        match egress.expect_sent().await {
            Sent::Datagram(object) => {
                sent.push((true, object.location.group_id, object.location.object_id));
            }
            Sent::Object(DataObject::SubgroupObject(object)) => {
                assert!(matches!(
                    object.forwarding,
                    ForwardingPreference::Subgroup { .. }
                ));
                sent.push((false, object.location.group_id, object.location.object_id));
            }
            Sent::Object(DataObject::SubgroupHeader(_)) | Sent::Closed => {}
            Sent::Reset(code) => panic!("a downstream stream was reset with {code:?}"),
            Sent::Object(
                DataObject::SubgroupObjectStart { .. } | DataObject::SubgroupPayload { .. },
            ) => unreachable!("the upstream sends whole objects"),
        }
    }
    sent.sort();
    sent
}

fn send_group(upstream: &UpstreamTrack, datagram: bool, group_id: u64) {
    if datagram {
        upstream.datagram(datagram_object(group_id, 0).object);
    } else {
        let stream = upstream.open_stream();
        stream.header(group_id);
        stream.object(0);
        stream.fin();
    }
}

async fn assert_relays_a_mixed_track(datagram_first: bool) {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let upstream = harness.start_track_ingest();

    // Act
    send_group(&upstream, datagram_first, 0);
    send_group(&upstream, !datagram_first, 1);
    send_group(&upstream, datagram_first, 2);
    let sent = sent_objects(&mut egress, 3).await;

    // Assert: each object keeps its preference and the subscription goes on
    let mut expected = vec![
        (datagram_first, 0, 0),
        (!datagram_first, 1, 0),
        (datagram_first, 2, 0),
    ];
    expected.sort();
    assert_eq!(sent, expected);
    assert!(!harness.is_track_malformed());
    egress
        .assert_no_publish_done_within(NO_PUBLISH_DONE_WINDOW)
        .await;
}

#[tokio::test]
async fn a_subgroup_stream_after_a_datagram_is_relayed() {
    assert_relays_a_mixed_track(true).await;
}

#[tokio::test]
async fn a_datagram_after_a_subgroup_stream_is_relayed() {
    assert_relays_a_mixed_track(false).await;
}

#[tokio::test]
async fn a_single_preference_subscription_that_began_on_a_stream_gets_datagrams_on_streams() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_single_preference_egress().await;
    let upstream = harness.start_track_ingest();
    send_group(&upstream, false, 0);
    let first = [
        egress.expect_sent().await,
        egress.expect_sent().await,
        egress.expect_sent().await,
    ];
    let mut ending_datagram = datagram_object(1, 3).object;
    ending_datagram.ends_group = true;

    // Act
    upstream.datagram(ending_datagram.clone());
    let sent = [
        egress.expect_sent().await,
        egress.expect_sent().await,
        egress.expect_sent().await,
    ];

    // Assert: draft-14 §10: the datagram goes alone on a converted subgroup and its End of Group moves to the header
    assert!(matches!(
        first,
        [
            Sent::Object(DataObject::SubgroupHeader(_)),
            Sent::Object(DataObject::SubgroupObject(_)),
            Sent::Closed
        ]
    ));
    let [
        Sent::Object(DataObject::SubgroupHeader(header)),
        Sent::Object(DataObject::SubgroupObject(object)),
        Sent::Closed,
    ] = &sent
    else {
        panic!("the datagram should go alone on a subgroup stream, got {sent:?}");
    };
    let expected_header = SubgroupStart {
        ends_group: true,
        ..subgroup_start(1, CONVERTED_DATAGRAM_SUBGROUPS + 3, false)
    };
    assert_eq!(*header, expected_header);
    assert_eq!(object.location, ending_datagram.location);
    assert_eq!(
        object.forwarding,
        ForwardingPreference::Subgroup {
            subgroup_id: CONVERTED_DATAGRAM_SUBGROUPS + 3
        }
    );
    assert!(!harness.is_track_malformed());
}

#[tokio::test]
async fn a_single_preference_subscription_that_began_with_a_datagram_gets_stream_objects_as_datagrams()
 {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_single_preference_egress().await;
    let upstream = harness.start_track_ingest();
    send_group(&upstream, true, 0);
    let first = egress.expect_sent().await;

    // Act
    let stream = upstream.open_stream();
    stream.header(1);
    stream.object(0);
    stream.object(1);
    stream.fin();
    let sent = [egress.expect_sent().await, egress.expect_sent().await];

    // Assert: no stream is opened for the subgroup
    assert!(matches!(first, Sent::Datagram(_)));
    for (object_id, sent) in sent.iter().enumerate() {
        let Sent::Datagram(object) = sent else {
            panic!("stream objects should go as datagrams, got {sent:?}");
        };
        assert_eq!(object.location.group_id, 1);
        assert_eq!(object.location.object_id, object_id as u64);
        assert_eq!(object.forwarding, ForwardingPreference::Datagram);
    }
    egress
        .assert_nothing_sent_within(NO_PUBLISH_DONE_WINDOW)
        .await;
    assert!(!harness.is_track_malformed());
}

/// The Subgroup IDs of the next `streams` streams sent downstream, each
/// one header, one object and a FIN.
async fn subgroup_ids_of_streams(egress: &mut EgressRunnerHandle, streams: usize) -> Vec<u64> {
    let mut subgroup_ids = Vec::new();
    for _ in 0..streams {
        let sent = [
            egress.expect_sent().await,
            egress.expect_sent().await,
            egress.expect_sent().await,
        ];
        let [
            Sent::Object(DataObject::SubgroupHeader(header)),
            Sent::Object(DataObject::SubgroupObject(object)),
            Sent::Closed,
        ] = &sent
        else {
            panic!("expected one object on a subgroup stream, got {sent:?}");
        };
        assert_eq!(
            object.forwarding,
            ForwardingPreference::Subgroup {
                subgroup_id: header.subgroup_id
            }
        );
        subgroup_ids.push(header.subgroup_id);
    }
    subgroup_ids
}

#[tokio::test]
async fn a_converted_datagram_goes_on_a_reserved_subgroup_apart_from_the_real_subgroup_of_its_object_id()
 {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_single_preference_egress().await;
    let upstream = harness.start_track_ingest();
    send_group(&upstream, false, 0);
    let stream = upstream.open_stream();
    stream.send(make_header_with(1, 3, false));
    stream.send(make_object_in_subgroup(1, 3, 0));
    stream.fin();
    let real = subgroup_ids_of_streams(&mut egress, 2).await;

    // Act
    upstream.datagram(datagram_object(1, 3).object);
    let converted = subgroup_ids_of_streams(&mut egress, 1).await;

    // Assert: draft-14 §2.5: a subgroup with two final objects is malformed, so the datagram of Object ID 3 must not close a second subgroup 3
    assert_eq!(real, vec![0, 3]);
    assert_eq!(converted, vec![CONVERTED_DATAGRAM_SUBGROUPS + 3]);
    assert!(!harness.is_track_malformed());
}

/// `(group, object, forwarding preference)` of each object live egress sent
/// downstream, the converted datagrams' subgroup streams included, sorted.
async fn sent_forwarding(
    egress: &mut EgressRunnerHandle,
    count: usize,
) -> Vec<(u64, u64, ForwardingPreference)> {
    let mut sent = Vec::new();
    while sent.len() < count {
        match egress.expect_sent().await {
            Sent::Datagram(object) | Sent::Object(DataObject::SubgroupObject(object)) => {
                sent.push((
                    object.location.group_id,
                    object.location.object_id,
                    object.forwarding,
                ));
            }
            Sent::Object(DataObject::SubgroupHeader(_)) | Sent::Closed => {}
            other => panic!("unexpected downstream step {other:?}"),
        }
    }
    sent.sort_by_key(|&(group_id, object_id, _)| (group_id, object_id));
    sent
}

/// The same for a FETCH of groups 0 and 1 from the cache to the session of
/// `harness.start_single_preference_egress`.
async fn fetched_forwarding(harness: &RelayHarness) -> Vec<(u64, u64, ForwardingPreference)> {
    let fetch = harness.start_fetch_reporting_unknown(
        location(0, 0),
        location(2, 0),
        moqt::fetch::UnknownRange::Marked,
    );
    fetch
        .finished()
        .await
        .into_iter()
        .filter_map(|sent| match sent {
            FetchSent::Object(object) => Some((
                object.location.group_id,
                object.location.object_id,
                object.forwarding,
            )),
            _ => None,
        })
        .collect()
}

async fn assert_fetch_matches_the_live_conversion(datagram_first: bool) {
    // Arrange: group 1 is of the other kind, so live egress converted it
    let harness = RelayHarness::new();
    let mut egress = harness.start_single_preference_egress().await;
    let upstream = harness.start_track_ingest();
    send_group(&upstream, datagram_first, 0);
    let mut live = sent_forwarding(&mut egress, 1).await;
    send_group(&upstream, !datagram_first, 1);
    live.extend(sent_forwarding(&mut egress, 1).await);
    harness.wait_largest_location(location(1, 0)).await;

    // Act
    let fetched = fetched_forwarding(&harness).await;

    // Assert: draft-14 §8.1, a copy with another forwarding preference or subgroup is a conflicting duplicate
    assert_eq!(fetched, live);
    let converted = if datagram_first {
        ForwardingPreference::Datagram
    } else {
        ForwardingPreference::Subgroup {
            subgroup_id: CONVERTED_DATAGRAM_SUBGROUPS,
        }
    };
    assert_eq!(live[1], (1, 0, converted));
}

#[tokio::test]
async fn a_fetch_gives_a_session_that_began_with_a_datagram_its_stream_objects_as_datagrams() {
    assert_fetch_matches_the_live_conversion(true).await;
}

#[tokio::test]
async fn a_fetch_gives_a_session_that_began_on_a_stream_its_datagrams_on_their_converted_subgroups()
{
    assert_fetch_matches_the_live_conversion(false).await;
}

#[tokio::test]
async fn a_later_subscription_of_the_session_keeps_the_preference_its_first_one_fixed() {
    // Arrange: the first subscription began with a datagram; the later one
    // begins after a stream group
    let harness = RelayHarness::new();
    let upstream = harness.start_track_ingest();
    let mut first = harness.start_single_preference_egress().await;
    send_group(&upstream, true, 0);
    sent_forwarding(&mut first, 1).await;
    drop(first);
    send_group(&upstream, false, 1);
    harness.wait_largest_location(location(1, 0)).await;
    let mut later = harness
        .start_single_preference_egress_after(Some(location(1, 0)))
        .await;

    // Act
    send_group(&upstream, false, 2);
    let sent = sent_forwarding(&mut later, 1).await;

    // Assert: draft-14 §8.1, a FETCH copy the session takes must agree with what both subscriptions sent
    assert_eq!(sent, vec![(2, 0, ForwardingPreference::Datagram)]);
}
