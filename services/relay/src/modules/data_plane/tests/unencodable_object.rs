//! A draft-18 publisher's value of 2^62 or more toward a draft-14
//! subscriber, whose varints stop below it.

use std::time::Duration;

use moqt::{
    object::{DataStreamResetCode, Object, SubgroupPart},
    publish::PublishDoneCode,
};

use crate::modules::test_support::relay_harness::{
    DRAFT14_VARINT_LIMIT, EgressRunnerHandle, FetchSent, RelayHarness, Sent,
    fixtures::{
        cached_object::{datagram_object, stream_object},
        data_object::make_header_with,
        location,
    },
};

const NO_PUBLISH_DONE_WINDOW: Duration = Duration::from_millis(200);

fn object(group_id: u64, object_id: u64) -> Object {
    stream_object(group_id, object_id).object
}

fn with_unencodable_extension(mut object: Object) -> Object {
    object
        .extension_headers
        .push_prior_group_id_gap(DRAFT14_VARINT_LIMIT);
    object
}

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
async fn a_subgroup_object_with_an_unencodable_extension_is_skipped_and_the_rest_goes_on_a_new_stream()
 {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_draft14_egress().await;
    let upstream = harness.open_upstream_stream();

    // Act
    upstream.send(make_header_with(0, 0, false));
    upstream.send(SubgroupPart::Object(object(0, 0)));
    upstream.send(SubgroupPart::Object(with_unencodable_extension(object(
        0, 1,
    ))));
    upstream.send(SubgroupPart::Object(object(0, 2)));
    upstream.fin();

    // Assert: draft-14 §10.4.2: a skipped object resets the stream and the rest of the subgroup opens a new one
    assert_eq!(
        next_steps(&mut egress, 6).await,
        vec![
            "header 0",
            "0/0",
            "reset InternalError",
            "header 0",
            "0/2",
            "fin"
        ]
    );
    egress
        .assert_no_publish_done_within(NO_PUBLISH_DONE_WINDOW)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unencodable_object_id_ends_the_subscription_with_internal_error() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_draft14_egress().await;
    let upstream = harness.open_upstream_stream();

    // Act
    upstream.send(make_header_with(0, 0, false));
    upstream.send(SubgroupPart::Object(object(0, 0)));
    upstream.send(SubgroupPart::Object(object(0, DRAFT14_VARINT_LIMIT)));

    // Assert
    assert_eq!(
        next_steps(&mut egress, 3).await,
        vec!["header 0", "0/0", "reset InternalError"]
    );
    let publish_done = egress.expect_publish_done().await;
    assert_eq!(publish_done.status_code, PublishDoneCode::InternalError);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unencodable_group_id_ends_the_subscription_with_internal_error() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_draft14_egress().await;
    let upstream = harness.open_upstream_stream();

    // Act
    upstream.send(make_header_with(DRAFT14_VARINT_LIMIT, 0, false));
    upstream.send(SubgroupPart::Object(object(DRAFT14_VARINT_LIMIT, 0)));

    // Assert: the stream opened for the header is reset
    assert_eq!(
        next_steps(&mut egress, 1).await,
        vec!["reset InternalError"]
    );
    let publish_done = egress.expect_publish_done().await;
    assert_eq!(publish_done.status_code, PublishDoneCode::InternalError);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unencodable_subgroup_id_resets_only_that_subgroup() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_draft14_egress().await;
    let unencodable = harness.open_upstream_stream();
    let next = harness.open_upstream_stream();
    let mut in_unencodable_subgroup = object(0, 0);
    in_unencodable_subgroup.forwarding = moqt::object::ForwardingPreference::Subgroup {
        subgroup_id: DRAFT14_VARINT_LIMIT,
    };

    // Act
    unencodable.send(make_header_with(0, DRAFT14_VARINT_LIMIT, false));
    unencodable.send(SubgroupPart::Object(in_unencodable_subgroup));
    unencodable.fin();
    let first = next_steps(&mut egress, 1).await;
    next.send(make_header_with(1, 0, false));
    next.send(SubgroupPart::Object(object(1, 0)));
    next.fin();

    // Assert
    assert_eq!(first, vec!["reset InternalError"]);
    assert_eq!(
        next_steps(&mut egress, 3).await,
        vec!["header 1", "1/0", "fin"]
    );
    egress
        .assert_no_publish_done_within(NO_PUBLISH_DONE_WINDOW)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_datagram_with_an_unencodable_extension_is_skipped_and_the_next_is_sent() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_draft14_egress().await;
    let upstream = harness.start_track_ingest();

    // Act
    upstream.datagram(with_unencodable_extension(datagram_object(0, 0).object));
    upstream.datagram(datagram_object(0, 1).object);

    // Assert
    assert_eq!(next_steps(&mut egress, 1).await, vec!["0/1"]);
    egress
        .assert_no_publish_done_within(NO_PUBLISH_DONE_WINDOW)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_datagram_of_an_unencodable_group_id_ends_the_subscription_with_internal_error() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_draft14_egress().await;
    let upstream = harness.start_track_ingest();

    // Act
    upstream.datagram(datagram_object(DRAFT14_VARINT_LIMIT, 0).object);

    // Assert
    let publish_done = egress.expect_publish_done().await;
    assert_eq!(publish_done.status_code, PublishDoneCode::InternalError);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fetch_reaching_an_unencodable_object_is_reset_with_internal_error() {
    // Arrange
    let harness = RelayHarness::new();
    let upstream = harness.open_upstream_stream();
    upstream.send(make_header_with(0, 0, false));
    upstream.send(SubgroupPart::Object(object(0, 0)));
    upstream.send(SubgroupPart::Object(with_unencodable_extension(object(
        0, 1,
    ))));
    upstream.send(SubgroupPart::Object(object(0, 2)));
    upstream.fin();
    harness.wait_group_closed(0).await;

    // Act
    let mut fetch = harness.start_draft14_fetch(location(0, 0), location(0, 2));

    // Assert: draft-14 §9.16 reads a gap as objects that do not exist, so the fetch ends
    fetch.expect_objects(&[(0, 0)]).await;
    assert!(matches!(
        fetch.expect_end().await,
        FetchSent::Reset(DataStreamResetCode::InternalError)
    ));
}
