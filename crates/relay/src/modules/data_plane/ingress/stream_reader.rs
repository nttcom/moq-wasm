use moqt::object::{ObjectStatus, SubgroupStart};
use tokio::{sync::mpsc, task::JoinSet};
use tracing::{Instrument, Span};

use crate::modules::{
    data_plane::{
        cache::{
            cached_object::CachedObject,
            subgroup_key::SubgroupKey,
            track_cache::{OpenSubgroupGuard, TrackCache},
        },
        ingress::track_ingest_task::TrackIngest,
    },
    domain::{session_id::SessionId, track_key::TrackKey},
    session::{
        data_object::DataObject,
        data_receiver::stream_receiver::{StreamReceiver, StreamReceiverFactory},
        session_event::SessionEvent,
    },
};

struct ReceivedHeader {
    start: SubgroupStart,
    prev_object_id: Option<u64>,
}

pub(crate) async fn accept_streams(
    mut ingest: TrackIngest,
    mut factory: Box<dyn StreamReceiverFactory>,
    track_span: Span,
) {
    let mut readers = JoinSet::new();
    loop {
        tokio::select! {
            _ = ingest.stop_receiver.changed() => {
                tracing::info!(track_key = %ingest.track_key, "stream ingress factory stopped");
                break;
            }
            receiver = factory.next() => {
                let receiver = match receiver {
                    Ok(receiver) => receiver,
                    Err(error) => {
                        if error.is::<moqt::object::MalformedTrackError>() {
                            ingest.cache.mark_malformed();
                            report_malformed_track(
                                &Span::current(),
                                &ingest.session_event_sender,
                                ingest.publisher_session_id,
                                &ingest.track_key,
                            );
                        }
                        break;
                    }
                };
                let span = tracing::info_span!(
                    parent: &track_span,
                    "relay.dataplane.ingress.stream",
                    track_key = %ingest.track_key,
                    group_id = tracing::field::Empty,
                    subgroup_id = tracing::field::Empty,
                    end_reason = tracing::field::Empty,
                );
                readers.spawn(read_stream(ingest.clone(), receiver).instrument(span));
            }
            Some(result) = readers.join_next() => {
                if let Err(e) = result {
                    tracing::error!("stream read task panicked: {:?}", e);
                }
            }
        }
    }
    readers.detach_all();
}

pub(crate) async fn read_stream(ingest: TrackIngest, mut receiver: Box<dyn StreamReceiver>) {
    let TrackIngest {
        track_key,
        publisher_session_id,
        cache,
        session_event_sender,
        mut stop_receiver,
    } = ingest;
    let span = Span::current();
    let mut header: Option<ReceivedHeader> = None;
    let mut ingest: Option<OpenSubgroupGuard<'_>> = None;
    loop {
        let receive_result = tokio::select! {
            _ = stop_receiver.changed() => {
                span.record("end_reason", "stopped");
                tracing::info!(%track_key, "stream reader stopped");
                return;
            }
            result = receiver.receive_object() => result,
        };

        match receive_result {
            Ok(Some(DataObject::SubgroupHeader(start))) => {
                span.record("group_id", start.group_id);
                ingest = Some(open_subgroup(&cache, &span, &start));
                header = Some(ReceivedHeader {
                    start,
                    prev_object_id: None,
                });
            }
            Ok(Some(DataObject::SubgroupObject(object))) => {
                let (Some(header), Some(current)) = (header.as_mut(), ingest.as_mut()) else {
                    span.record("end_reason", "object_before_header");
                    tracing::error!(%track_key, "subgroup object received before its header");
                    return;
                };
                header.prev_object_id = Some(object.location.object_id);
                let end_reason = match object.status {
                    ObjectStatus::EndOfGroup => Some("end_of_group"),
                    ObjectStatus::EndOfTrack => Some("end_of_track"),
                    _ => None,
                };
                if current.insert(CachedObject::from_object(object)).is_err() {
                    report_malformed_track(
                        &span,
                        &session_event_sender,
                        publisher_session_id,
                        &track_key,
                    );
                    return;
                }
                if let Some(end_reason) = end_reason {
                    span.record("end_reason", end_reason);
                    if let Some(ingest) = ingest.take() {
                        ingest.finish();
                    }
                    return;
                }
            }
            Ok(Some(DataObject::Datagram(_))) => {
                span.record("end_reason", "unexpected_datagram");
                tracing::error!(%track_key, "datagram received on a subgroup stream");
                return;
            }
            Ok(None) => {
                span.record("end_reason", "fin");
                if let (Some(header), Some(ingest)) = (&header, &ingest)
                    && header.start.ends_group
                {
                    let end_of_group_id = header.prev_object_id.map_or(0, |id| id + 1);
                    if ingest
                        .insert(CachedObject::end_of_group(&header.start, end_of_group_id))
                        .is_err()
                    {
                        report_malformed_track(
                            &span,
                            &session_event_sender,
                            publisher_session_id,
                            &track_key,
                        );
                        return;
                    }
                }
                if let Some(ingest) = ingest.take() {
                    ingest.finish();
                }
                tracing::debug!(%track_key, "stream finished");
                return;
            }
            Err(moqt::object::StreamReceiveError::Closed(error)) => {
                span.record("end_reason", "transport_closed");
                tracing::info!(%track_key, %error, "stream transport closed");
                return;
            }
            Err(moqt::object::StreamReceiveError::Decode(error)) => {
                span.record("end_reason", "decode_error");
                tracing::error!(%track_key, %error, "failed to decode stream data");
                return;
            }
            Err(moqt::object::StreamReceiveError::ProtocolViolation(error)) => {
                span.record("end_reason", "protocol_violation");
                tracing::error!(%track_key, %error, "protocol violation on the stream");
                let _ = session_event_sender.send(SessionEvent::protocol_violation_detected(
                    publisher_session_id,
                    error,
                ));
                return;
            }
        }
    }
}

fn report_malformed_track(
    span: &Span,
    session_event_sender: &mpsc::UnboundedSender<SessionEvent>,
    publisher_session_id: SessionId,
    track_key: &TrackKey,
) {
    span.record("end_reason", "malformed_track");
    tracing::warn!(%track_key, "malformed track detected; stopping stream ingest");
    let _ = session_event_sender.send(SessionEvent::malformed_track_detected(
        publisher_session_id,
        track_key.clone(),
    ));
}

fn open_subgroup<'a>(
    cache: &'a TrackCache,
    span: &Span,
    start: &SubgroupStart,
) -> OpenSubgroupGuard<'a> {
    span.record("subgroup_id", start.subgroup_id);
    cache.open_subgroup(SubgroupKey::Stream {
        group_id: start.group_id,
        subgroup_id: start.subgroup_id,
    })
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use moqt::object::ObjectStatus;

    use super::*;
    use crate::modules::{
        data_plane::cache::track_cache::NextObject,
        session::session_event::EventKind,
        test_support::relay_harness::{
            FailingUpstream, PUBLISHER_SESSION_ID, RelayHarness, UpstreamSubgroupStream,
            fixtures::{
                cached_object::stream_key,
                data_object::{
                    make_header, make_header_with, make_object_in_subgroup, make_payload_object,
                    make_status_object,
                },
                location,
            },
        },
    };

    fn payload(object_id: u64) -> DataObject {
        make_payload_object(0, object_id, Bytes::from_static(b"payload"))
    }

    fn send_all(upstream_stream: &UpstreamSubgroupStream, objects: Vec<DataObject>) {
        for object in objects {
            upstream_stream.send(object);
        }
    }

    #[tokio::test]
    async fn end_of_group_status_closes_subgroup_and_notifies() {
        // Arrange
        let harness = RelayHarness::new();
        let mut subgroup_opened_receiver = harness.subscribe_subgroup_opened();
        let mut upstream_stream = harness.open_upstream_stream();
        // Act
        send_all(
            &upstream_stream,
            vec![
                make_header(0),
                payload(0),
                make_status_object(0, 1, ObjectStatus::EndOfGroup),
            ],
        );
        upstream_stream.wait_reader_end().await;
        // Assert
        assert_eq!(
            subgroup_opened_receiver.try_recv().map(|run| run.key),
            Ok(stream_key(0))
        );
        assert_eq!(
            harness.cached_object_ids(stream_key(0)).await,
            vec![(0, ObjectStatus::Normal), (1, ObjectStatus::EndOfGroup)]
        );
        assert!(matches!(
            harness.subgroup_end_after(stream_key(0), 1).await,
            NextObject::Finished
        ));
    }

    async fn assert_open_subgroup_ends_on(
        terminate: impl FnOnce(&UpstreamSubgroupStream),
        finished: bool,
    ) {
        // Arrange
        let harness = RelayHarness::new();
        let mut subgroup_opened_receiver = harness.subscribe_subgroup_opened();
        let mut upstream_stream = harness.open_upstream_stream();
        send_all(&upstream_stream, vec![make_header(0), payload(0)]);
        // Act
        terminate(&upstream_stream);
        upstream_stream.wait_reader_end().await;
        // Assert
        assert_eq!(
            subgroup_opened_receiver.try_recv().map(|run| run.key),
            Ok(stream_key(0))
        );
        let end = harness.subgroup_end_after(stream_key(0), 0).await;
        assert_eq!(
            matches!(end, NextObject::Finished),
            finished,
            "unexpected end: {end:?}"
        );
    }

    #[tokio::test]
    async fn fin_finishes_the_open_subgroup() {
        assert_open_subgroup_ends_on(UpstreamSubgroupStream::fin, true).await;
    }

    #[tokio::test]
    async fn transport_close_aborts_the_open_subgroup() {
        assert_open_subgroup_ends_on(UpstreamSubgroupStream::reset, false).await;
    }

    #[tokio::test]
    async fn decode_failure_aborts_the_open_subgroup() {
        assert_open_subgroup_ends_on(UpstreamSubgroupStream::decode_error, false).await;
    }

    #[tokio::test]
    async fn stop_signal_closes_open_subgroup() {
        // Arrange: the stream stays open after two objects
        let harness = RelayHarness::new();
        let mut upstream_stream = harness.open_upstream_stream();
        send_all(&upstream_stream, vec![make_header(0), payload(0)]);
        harness.wait_largest_location(location(0, 0)).await;
        // Act
        harness.stop_ingest();
        upstream_stream.wait_reader_end().await;
        // Assert: a stopped reader cannot vouch for the subgroup's tail
        assert!(matches!(
            harness.subgroup_end_after(stream_key(0), 0).await,
            NextObject::Aborted
        ));
    }

    #[tokio::test]
    async fn end_of_group_header_type_synthesizes_status_object_on_fin() {
        // Arrange: Type 0x18 header (last object before FIN ends the group)
        let harness = RelayHarness::new();
        let mut upstream_stream = harness.open_upstream_stream();
        // Act
        send_all(
            &upstream_stream,
            vec![make_header_with(0, 0, true), payload(0), payload(1)],
        );
        upstream_stream.fin();
        upstream_stream.wait_reader_end().await;
        // Assert: the End of Group becomes canonical data at last_id + 1
        assert_eq!(
            harness.cached_object_ids(stream_key(0)).await,
            vec![
                (0, ObjectStatus::Normal),
                (1, ObjectStatus::Normal),
                (2, ObjectStatus::EndOfGroup)
            ]
        );
    }

    #[tokio::test]
    async fn conflicting_synthesized_end_of_group_reports_the_malformed_track() {
        // Arrange: subgroup 0 already holds a Normal object 1; a Type 0x1C stream
        // for subgroup 1 ends after object 0, so its End of Group lands on id 1
        let mut harness = RelayHarness::new();
        let mut first_stream = harness.open_upstream_stream();
        send_all(&first_stream, vec![make_header(0), payload(0), payload(1)]);
        first_stream.fin();
        first_stream.wait_reader_end().await;
        let mut second_stream = harness.open_upstream_stream();
        // Act
        send_all(
            &second_stream,
            vec![
                make_header_with(0, 1, true),
                make_object_in_subgroup(0, 1, 0),
            ],
        );
        second_stream.fin();
        second_stream.wait_reader_end().await;
        // Assert
        harness.wait_track_malformed().await;
        let event = harness.expect_session_event().await;
        assert!(matches!(event.kind, EventKind::MalformedTrackDetected(_)));
    }

    #[tokio::test]
    async fn a_datagram_on_the_stream_track_reports_the_malformed_track() {
        // Arrange
        let mut harness = RelayHarness::new();
        // Act
        harness
            .run_stream_ingest(FailingUpstream(|| moqt::object::MalformedTrackError.into()))
            .await;
        // Assert
        harness.wait_track_malformed().await;
        let event = harness.expect_session_event().await;
        assert!(matches!(event.kind, EventKind::MalformedTrackDetected(_)));
    }

    #[tokio::test]
    async fn an_ended_upstream_subscription_does_not_mark_the_track_malformed() {
        // Arrange
        let harness = RelayHarness::new();
        // Act
        harness
            .run_stream_ingest(FailingUpstream(|| anyhow::anyhow!("Stream channel closed")))
            .await;
        // Assert
        assert!(!harness.is_track_malformed());
    }

    #[tokio::test]
    async fn unknown_object_status_terminates_the_publisher_session() {
        // Arrange: moqt rejects a status draft-14 §10.2.1.1 does not define
        let mut harness = RelayHarness::new();
        let mut upstream_stream = harness.open_upstream_stream();
        // Act
        send_all(&upstream_stream, vec![make_header(0), payload(0)]);
        upstream_stream.protocol_violation();
        upstream_stream.wait_reader_end().await;
        // Assert: the object is not cached and the session is reported for termination
        assert_eq!(
            harness.cached_object_ids(stream_key(0)).await,
            vec![(0, ObjectStatus::Normal)]
        );
        let event = harness.expect_session_event().await;
        assert_eq!(event.session_id, PUBLISHER_SESSION_ID);
        assert!(matches!(
            event.kind,
            EventKind::ProtocolViolationDetected { .. }
        ));
    }

    #[tokio::test]
    async fn end_of_group_header_type_does_not_synthesize_on_reset() {
        // Arrange: a reset stream cannot tell where the group ended (§10.4.2)
        let harness = RelayHarness::new();
        let mut upstream_stream = harness.open_upstream_stream();
        // Act
        send_all(
            &upstream_stream,
            vec![make_header_with(0, 0, true), payload(0)],
        );
        upstream_stream.reset();
        upstream_stream.wait_reader_end().await;
        // Assert
        assert_eq!(
            harness.cached_object_ids(stream_key(0)).await,
            vec![(0, ObjectStatus::Normal)]
        );
    }
}
