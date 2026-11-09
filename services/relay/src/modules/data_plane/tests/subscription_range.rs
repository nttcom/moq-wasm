use std::time::Duration;

use moqt::{object::DataStreamResetCode, publish::PublishDoneCode, subscribe::FilterType};

use crate::modules::{
    domain::delivery_control::DeliveryControl,
    session::data_object::DataObject,
    test_support::relay_harness::{
        RelayHarness, Sent, downstream_object_ids, fixtures::location, receive_objects_until_close,
        receive_objects_until_end,
    },
};

const NOTHING_SENT_WINDOW: Duration = Duration::from_millis(200);

fn end_group_update(end_group: u64) -> DeliveryControl {
    DeliveryControl::from_subscribe_update(true, location(0, 0), end_group)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn absolute_range_delivers_its_end_group_and_nothing_after_it() {
    // Arrange: group 1 is downstream before group 2 opens; upstream streams are read concurrently, so group 2 could otherwise enter the cache first
    let harness = RelayHarness::new();
    let filter_type = FilterType::AbsoluteRange {
        location: location(1, 0),
        end_group: 1,
    };
    let mut egress = harness.start_egress_with_filter(filter_type, None).await;

    let last_group = harness.open_upstream_stream();
    last_group.header(1);
    last_group.object(0);
    last_group.fin();
    let objects = receive_objects_until_close(&mut egress).await;

    // Act
    let past_end = harness.open_upstream_stream();
    past_end.header(2);
    past_end.object(0);
    past_end.fin();

    // Assert
    assert!(matches!(
        objects.first(),
        Some(DataObject::SubgroupHeader(header)) if header.group_id == 1
    ));
    let publish_done = egress.expect_publish_done().await;
    assert_eq!(publish_done.status_code, PublishDoneCode::SubscriptionEnded);
    assert_eq!(publish_done.stream_count, 1);
    egress.assert_nothing_more_sent();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_moving_end_group_below_an_open_group_resets_its_stream() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(3);
    upstream_stream.object(0);
    egress.expect_stream_priority().await;
    egress.expect_sent_object().await;
    egress.expect_sent_object().await;

    // Act
    egress.apply_subscribe_update(end_group_update(3));
    upstream_stream.object(1);

    // Assert
    let (objects, end) = receive_objects_until_end(&mut egress).await;
    assert!(objects.is_empty(), "no object of a reset group follows");
    assert!(
        matches!(end, Sent::Reset(DataStreamResetCode::Cancelled)),
        "the stream of a group outside the range must be reset, got {end:?}"
    );
    assert_eq!(egress.delivery_counters().streams_reset, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_moving_end_group_keeps_delivering_the_groups_inside_it() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(3);
    upstream_stream.object(0);
    egress.expect_stream_priority().await;

    // Act
    egress.apply_subscribe_update(end_group_update(4));
    upstream_stream.object(1);
    upstream_stream.fin();
    let past_end = harness.open_upstream_stream();
    past_end.header(4);
    past_end.object(0);
    past_end.fin();

    // Assert
    let objects = receive_objects_until_close(&mut egress).await;
    assert_eq!(downstream_object_ids(&objects), vec![0, 1]);
    let publish_done = egress.expect_publish_done().await;
    assert_eq!(publish_done.status_code, PublishDoneCode::SubscriptionEnded);
    assert_eq!(publish_done.stream_count, 1);
    egress.assert_nothing_more_sent();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_moving_start_past_an_open_group_resets_its_stream() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(3);
    upstream_stream.object(0);
    egress.expect_stream_priority().await;

    // Act
    egress.apply_subscribe_update(DeliveryControl::from_subscribe_update(
        true,
        location(4, 0),
        0,
    ));
    upstream_stream.object(1);

    // Assert
    let (_, end) = receive_objects_until_end(&mut egress).await;
    assert!(
        matches!(end, Sent::Reset(DataStreamResetCode::Cancelled)),
        "the stream of a group before the new start must be reset, got {end:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn widening_update_leaves_the_end_group_in_force() {
    // Arrange
    let harness = RelayHarness::new();
    let filter_type = FilterType::AbsoluteRange {
        location: location(0, 0),
        end_group: 1,
    };
    let mut egress = harness.start_egress_with_filter(filter_type, None).await;

    // Act
    egress.apply_subscribe_update(end_group_update(0));
    let past_end = harness.open_upstream_stream();
    past_end.header(2);
    past_end.object(0);
    past_end.fin();

    // Assert
    let publish_done = egress.expect_publish_done().await;
    assert_eq!(publish_done.status_code, PublishDoneCode::SubscriptionEnded);
    assert_eq!(
        publish_done.stream_count, 0,
        "group 2 lies past the End Group"
    );
    egress.assert_nothing_more_sent();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publish_done_follows_the_close_of_the_last_group_in_range() {
    // Arrange
    let harness = RelayHarness::new();
    let filter_type = FilterType::AbsoluteRange {
        location: location(1, 0),
        end_group: 1,
    };
    let mut egress = harness.start_egress_with_filter(filter_type, None).await;
    let last_group = harness.open_upstream_stream();
    last_group.header(1);
    last_group.object(0);
    egress.expect_stream_priority().await;
    let past_end = harness.open_upstream_stream();
    past_end.header(2);
    past_end.object(0);
    egress
        .assert_no_publish_done_within(NOTHING_SENT_WINDOW)
        .await;

    // Act
    last_group.object(1);
    last_group.fin();

    // Assert
    let objects = receive_objects_until_close(&mut egress).await;
    assert_eq!(downstream_object_ids(&objects), vec![0, 1]);
    let publish_done = egress.expect_publish_done().await;
    assert_eq!(publish_done.status_code, PublishDoneCode::SubscriptionEnded);
    assert_eq!(publish_done.stream_count, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_ending_the_range_sends_publish_done_after_resetting_the_open_stream() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let upstream_stream = harness.open_upstream_stream();
    upstream_stream.header(3);
    upstream_stream.object(0);
    egress.expect_stream_priority().await;

    // Act
    egress.apply_subscribe_update(end_group_update(3));

    // Assert
    let (_, end) = receive_objects_until_end(&mut egress).await;
    assert!(matches!(end, Sent::Reset(DataStreamResetCode::Cancelled)));
    let publish_done = egress.expect_publish_done().await;
    assert_eq!(publish_done.status_code, PublishDoneCode::SubscriptionEnded);
    assert_eq!(publish_done.stream_count, 1);
}
