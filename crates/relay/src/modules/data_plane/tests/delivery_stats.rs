use crate::modules::test_support::relay_harness::{
    RelayHarness, Sent, ordered_payload, receive_objects_until_close, receive_objects_until_end,
};

#[tokio::test]
async fn delivered_payload_bytes_are_counted() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let upstream_stream = harness.open_upstream_stream();

    // Act
    upstream_stream.header(0);
    for index in 0..3 {
        upstream_stream.object(index);
    }
    upstream_stream.fin();
    receive_objects_until_close(&mut egress).await;

    // Assert
    let counters = egress.delivery_counters();
    let payload_bytes: usize = (0..3).map(|index| ordered_payload(index).len()).sum();
    assert_eq!(counters.bytes_sent, payload_bytes as u64);
    assert_eq!(counters.streams_reset, 0);
    assert!(counters.last_sent_received_at.is_some());
}

#[tokio::test]
async fn a_downstream_stream_reset_after_an_upstream_reset_is_counted() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress(None).await;
    let upstream_stream = harness.open_upstream_stream();

    // Act
    upstream_stream.header(0);
    upstream_stream.object(0);
    upstream_stream.reset();
    let (_, end) = receive_objects_until_end(&mut egress).await;

    // Assert
    assert!(
        matches!(end, Sent::Reset(_)),
        "expected a reset, got {end:?}"
    );
    assert_eq!(egress.delivery_counters().streams_reset, 1);
}
