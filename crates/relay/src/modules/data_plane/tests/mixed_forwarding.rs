use std::time::Duration;

use moqt::object::{ForwardingPreference, SubgroupStart};

use crate::modules::{
    session::data_object::DataObject,
    test_support::relay_harness::{
        EgressRunnerHandle, RelayHarness, Sent, UpstreamTrack,
        fixtures::{cached_object::datagram_object, data_object::subgroup_start},
    },
};

const NO_PUBLISH_DONE_WINDOW: Duration = Duration::from_millis(200);

/// `(datagram, group, object)` of each object sent downstream, sorted: the
/// datagram and stream senders run apart, so their order is not kept.
async fn sent_objects(egress: &mut EgressRunnerHandle, count: usize) -> Vec<(bool, u64, u64)> {
    let mut sent = Vec::new();
    while sent.len() < count {
        match egress.expect_sent().await {
            Sent::Object(DataObject::Datagram(object)) => {
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

    // Assert: draft-14 §10.4.4, the datagram goes alone in the subgroup of its
    // Object ID and its End of Group moves to the header
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
        ..subgroup_start(1, 3, false)
    };
    assert_eq!(*header, expected_header);
    assert_eq!(object.location, ending_datagram.location);
    assert_eq!(
        object.forwarding,
        ForwardingPreference::Subgroup { subgroup_id: 3 }
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
    assert!(matches!(first, Sent::Object(DataObject::Datagram(_))));
    for (object_id, sent) in sent.iter().enumerate() {
        let Sent::Object(DataObject::Datagram(object)) = sent else {
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
