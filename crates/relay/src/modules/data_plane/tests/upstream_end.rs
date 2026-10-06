use moqt::wire::publish_done_status_code;

use crate::modules::{
    data_plane::tests::harness::RelayHarness, domain::pub_sub_directory::entry::PublishDoneReason,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upstream_end_is_relayed_as_publish_done_counting_the_opened_streams() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let stream = harness.open_upstream_stream();
    stream.header(0);
    stream.object(0);
    egress.expect_stream_priority().await;

    // Act
    egress.end_upstream(PublishDoneReason::publisher_session_closed());

    // Assert
    let publish_done = egress.expect_publish_done().await;
    assert_eq!(
        publish_done.status_code,
        publish_done_status_code::TRACK_ENDED
    );
    assert_eq!(publish_done.request_id, 0);
    assert_eq!(publish_done.stream_count, 1);
}
