use bytes::Bytes;

use crate::modules::test_support::relay_harness::{
    RelayHarness, Sent, fixtures::cached_object::datagram_object, ordered_payload,
    receive_objects_until_close, receive_objects_until_end,
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

#[tokio::test]
async fn a_datagram_too_large_for_the_downstream_is_dropped_and_counted() {
    // Arrange
    let harness = RelayHarness::new();
    let mut egress = harness.start_egress_taking_datagrams_up_to(8).await;
    let upstream = harness.start_track_ingest();
    let mut oversized = datagram_object(0, 0).object;
    oversized.payload = Bytes::from(vec![0; 9]);

    // Act
    upstream.datagram(oversized);
    upstream.datagram(datagram_object(0, 1).object);
    let sent = egress.expect_sent().await;

    // Assert: draft-14 §10.3, draft-18 §11.3, the delivery goes on after the drop
    let Sent::Datagram(next) = sent else {
        panic!("expected the next datagram, got {sent:?}");
    };
    assert_eq!(next.location.object_id, 1);
    let counters = egress.delivery_counters();
    assert_eq!(counters.datagrams_dropped, 1);
    assert_eq!(counters.bytes_sent, next.payload.len() as u64);
}
