use moqt::object::DataStreamResetCode;
use std::time::Duration;

use crate::modules::session::data_object::DataObject;

use crate::modules::data_plane::cache::track_cache::NextObject;
use crate::modules::test_support::relay_harness::{
    OBJECT_COUNT, RelayHarness, Sent, assert_full_ordered_delivery, downstream_object_ids,
    fixtures::{
        cached_object::{FIXTURE_PRIORITY, stream_key, stream_object},
        data_object::{make_object_start, make_payload_piece},
        location,
    },
    ordered_payload, payloads_of, receive_objects_until_close, receive_objects_until_end,
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
            moqt::subscribe::FilterType::NextGroupStart,
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
    assert_eq!(downstream_object_ids(&objects), vec![0]);
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
async fn egress_joining_a_fetch_filled_subgroup_delivers_its_live_stream_opened_later() {
    // Arrange: a joining FETCH cached object 0 before the upstream stream opened
    let harness = RelayHarness::new();
    let _ = harness.cache().insert(stream_object(0, 0));
    let mut egress = harness.start_egress(Some(location(0, 0))).await;
    egress
        .assert_nothing_sent_within(Duration::from_millis(100))
        .await;

    // Act
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(0);
    upstream_stream.object_with_gap(1, 1);
    upstream_stream.fin();

    // Assert
    let objects = receive_objects_until_close(&mut egress).await;
    assert_eq!(downstream_object_ids(&objects), vec![1]);
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
    assert_eq!(downstream_object_ids(&objects), expected);
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
    assert_eq!(header.subgroup_id, 0);
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
        downstream_object_ids(&objects),
        vec![0, 1, 2],
        "objects received before the reset are still forwarded"
    );
    assert!(
        matches!(end, Sent::Reset(DataStreamResetCode::InternalError)),
        "a partial subgroup must be reset downstream with INTERNAL_ERROR, got {end:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscription_with_forward_off_opens_no_stream_for_a_new_group() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    egress.pause();

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
async fn while_forward_is_0_no_object_is_sent_even_on_a_stream_open_before_the_pause() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(0);
    upstream_stream.object(0);
    egress.expect_stream_priority().await;
    egress.expect_sent_object().await;
    egress.expect_sent_object().await;

    // Act
    egress.pause();
    upstream_stream.object(1);
    upstream_stream.fin();

    // Assert: draft-14 §5.1 / draft-18 §5.1, the open stream is reset (draft-18 §11.4.3)
    let (objects, end) = receive_objects_until_end(&mut egress).await;
    assert!(
        objects.is_empty(),
        "no object is sent at Forward 0: {objects:?}"
    );
    assert!(
        matches!(end, Sent::Reset(DataStreamResetCode::Cancelled)),
        "got {end:?}"
    );
    egress
        .assert_nothing_sent_within(Duration::from_millis(200))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_a_resume_an_object_in_a_subgroup_opened_while_paused_is_delivered_once() {
    // Arrange: group 0 opens while Forward is 0
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    egress.pause();
    let opened_while_paused = harness.open_upstream_stream();
    opened_while_paused.header(0);
    opened_while_paused.object(0);
    harness.wait_largest_location(location(0, 0)).await;
    egress
        .assert_nothing_sent_within(Duration::from_millis(200))
        .await;

    // Act: the update's answer carries Largest Object {0, 0}
    egress.resume(Some(location(0, 0)));
    opened_while_paused.object(1);
    opened_while_paused.fin();

    // Assert: draft-18 §11.4.3 lets the publisher reopen the subgroup on a new stream
    let objects = receive_objects_until_close(&mut egress).await;
    assert!(matches!(
        objects.first(),
        Some(DataObject::SubgroupHeader(header)) if header.group_id == 0
    ));
    assert_eq!(downstream_object_ids(&objects), vec![1]);
    egress
        .assert_nothing_sent_within(Duration::from_millis(200))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_a_resume_a_subgroup_open_before_the_pause_continues_on_a_new_stream_without_repeats()
{
    // Arrange: object 0 went out before the pause, object 1 arrived during it
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(0);
    upstream_stream.object(0);
    egress.expect_stream_priority().await;
    egress.expect_sent_object().await;
    egress.expect_sent_object().await;
    egress.pause();
    let (_, end) = receive_objects_until_end(&mut egress).await;
    assert!(matches!(end, Sent::Reset(DataStreamResetCode::Cancelled)));
    upstream_stream.object(1);
    harness.wait_largest_location(location(0, 1)).await;

    // Act
    egress.resume(Some(location(0, 1)));
    upstream_stream.object(2);
    upstream_stream.fin();

    // Assert: the objects up to the Largest Object at the resume are left to FETCH
    let objects = receive_objects_until_close(&mut egress).await;
    assert_eq!(downstream_object_ids(&objects), vec![2]);
    egress
        .assert_nothing_sent_within(Duration::from_millis(200))
        .await;
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
    reopened_stream.object_with_gap(3, 3);
    reopened_stream.object(4);
    reopened_stream.object(5);
    reopened_stream.fin();

    // Assert
    let objects = receive_objects_until_close(&mut egress).await;
    assert_eq!(downstream_object_ids(&objects), vec![3, 4, 5]);
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
    reopened_stream.object_with_gap(4, 4);
    reopened_stream.object(5);
    reopened_stream.fin();

    // Assert
    let reopened_objects = receive_objects_until_close(&mut egress).await;
    assert_eq!(downstream_object_ids(&reset_objects), vec![0, 1, 2]);
    assert!(
        matches!(reset_end, Sent::Reset(DataStreamResetCode::InternalError)),
        "got {reset_end:?}"
    );
    assert!(matches!(
        reopened_objects.first(),
        Some(DataObject::SubgroupHeader(_))
    ));
    assert_eq!(downstream_object_ids(&reopened_objects), vec![4, 5]);
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
        reopened_stream.object_with_gap(4, 4);
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
                (
                    Sent::Reset(DataStreamResetCode::InternalError),
                    Sent::Closed
                ) | (
                    Sent::Closed,
                    Sent::Reset(DataStreamResetCode::InternalError)
                )
            ),
            "the reset stream is reset and the reopened one finished, got {first_end:?} and {second_end:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lagging_publisher_does_not_hold_back_the_subgroup_another_publisher_finished() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let lagging_stream = harness.open_upstream_stream();
    lagging_stream.header(0);
    lagging_stream.object(0);

    // Act
    let finishing_stream = harness.open_upstream_stream();
    finishing_stream.header(0);
    finishing_stream.object(0);
    finishing_stream.object(1);
    finishing_stream.fin();

    // Assert
    let (objects, end) = receive_objects_until_end(&mut egress).await;
    assert_eq!(downstream_object_ids(&objects), vec![0, 1]);
    assert!(matches!(end, Sent::Closed), "expected a FIN, got {end:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_publisher_reset_is_not_relayed_while_another_publisher_finishes_the_subgroup() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let mut subgroup_opened_receiver = harness.subscribe_subgroup_opened();
    let resetting_stream = harness.open_upstream_stream();
    resetting_stream.header(0);
    let finishing_stream = harness.open_upstream_stream();
    finishing_stream.header(0);
    for _ in 0..2 {
        subgroup_opened_receiver
            .recv()
            .await
            .expect("both publishers should open the subgroup");
    }
    resetting_stream.object(0);
    finishing_stream.object(0);
    harness.wait_largest_location(location(0, 0)).await;

    // Act
    resetting_stream.reset();
    finishing_stream.object(1);
    finishing_stream.fin();

    // Assert
    let (objects, end) = receive_objects_until_end(&mut egress).await;
    assert_eq!(downstream_object_ids(&objects), vec![0, 1]);
    assert!(matches!(end, Sent::Closed), "expected a FIN, got {end:?}");
}

fn payload_piece(object: &DataObject) -> (&[u8], bool) {
    match object {
        DataObject::SubgroupPayload { data, last } => (&data[..], *last),
        other => panic!("expected a payload piece, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_arriving_object_is_forwarded_before_its_last_byte() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(0);
    upstream_stream.send(make_object_start(0, 0, 8));
    upstream_stream.send(make_payload_piece(b"keyf", false));

    // Act: the downstream receives while the second half is withheld
    let header = egress.expect_sent_object().await;
    let start = egress.expect_sent_object().await;
    let first_piece = egress.expect_sent_object().await;
    upstream_stream.send(make_payload_piece(b"rame", true));
    upstream_stream.fin();
    let (rest, end) = receive_objects_until_end(&mut egress).await;

    // Assert
    assert!(
        matches!(header, DataObject::SubgroupHeader(_)),
        "{header:?}"
    );
    assert!(
        matches!(
            &start,
            DataObject::SubgroupObjectStart { object, payload_len: 8 }
                if object.location == location(0, 0)
        ),
        "{start:?}"
    );
    assert_eq!(payload_piece(&first_piece), (&b"keyf"[..], false));
    assert_eq!(
        rest.iter().map(payload_piece).collect::<Vec<_>>(),
        vec![(&b"rame"[..], true)]
    );
    assert!(matches!(end, Sent::Closed), "{end:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upstream_reset_inside_a_payload_resets_the_downstream_stream() {
    // Arrange: the downstream has received the first half
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(0);
    upstream_stream.send(make_object_start(0, 0, 8));
    upstream_stream.send(make_payload_piece(b"keyf", false));
    for _ in 0..3 {
        egress.expect_sent_object().await;
    }

    // Act
    upstream_stream.reset();

    // Assert
    let (objects, end) = receive_objects_until_end(&mut egress).await;
    assert!(objects.is_empty(), "{objects:?}");
    assert!(
        matches!(end, Sent::Reset(DataStreamResetCode::InternalError)),
        "an object cut off upstream must be reset downstream, got {end:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subscriber_joining_inside_a_payload_receives_the_object_from_its_first_byte() {
    // Arrange: the first half has arrived before the subscription starts
    let harness = RelayHarness::new();
    let mut subgroup_opened = harness.subscribe_subgroup_opened();
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(0);
    upstream_stream.send(make_object_start(0, 0, 8));
    upstream_stream.send(make_payload_piece(b"keyf", false));
    let first_half_cached = async {
        subgroup_opened.recv().await.unwrap();
        let NextObject::Partial(partial) = harness
            .cache()
            .next_subgroup_object_or_wait(stream_key(0), 0, 0)
            .await
            .unwrap()
        else {
            panic!("expected the arriving object");
        };
        partial
            .subscribe()
            .wait_for(|progress| !progress.pieces.is_empty())
            .await
            .unwrap();
    };
    tokio::time::timeout(Duration::from_secs(5), first_half_cached)
        .await
        .expect("the first half reaches the cache as an arriving object");

    // Act: the second half is held back until the subscriber has the first,
    // so the object is still arriving when egress reaches it
    let mut egress = harness.start_egress(None).await;
    let header = egress.expect_sent_object().await;
    let start = egress.expect_sent_object().await;
    let first_piece = egress.expect_sent_object().await;
    upstream_stream.send(make_payload_piece(b"rame", true));
    upstream_stream.fin();
    let rest = receive_objects_until_close(&mut egress).await;

    // Assert
    assert!(
        matches!(header, DataObject::SubgroupHeader(_)),
        "{header:?}"
    );
    assert!(
        matches!(
            start,
            DataObject::SubgroupObjectStart { payload_len: 8, .. }
        ),
        "{start:?}"
    );
    assert_eq!(payload_piece(&first_piece), (&b"keyf"[..], false));
    assert_eq!(
        rest.iter().map(payload_piece).collect::<Vec<_>>(),
        vec![(&b"rame"[..], true)]
    );
}
