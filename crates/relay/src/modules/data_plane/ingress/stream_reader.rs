use moqt::ObjectStatus;
use tokio::{sync::mpsc, task::JoinSet};
use tracing::{Instrument, Span};

use crate::modules::{
    data_plane::{
        cache::{
            cached_object::{CachedObject, SubgroupHeaderFields},
            track_cache::{OpenSubgroupGuard, TrackCache},
        },
        ingress::track_ingest_task::TrackIngest,
    },
    session::{
        data_object::DataObject,
        data_receiver::stream_receiver::{StreamReceiver, StreamReceiverFactory},
        session_event::SessionEvent,
    },
    types::{SessionId, TrackKey},
};

/// What the SUBGROUP_HEADER told us; the subgroup id of Type 0x12/0x13/0x1A/0x1B
/// headers is only known once the first object arrives.
struct ReceivedHeader {
    group_id: u64,
    publisher_priority: u8,
    ends_group_on_fin: bool,
    prev_object_id: Option<u64>,
}

struct SubgroupIngest<'a> {
    header: SubgroupHeaderFields,
    open: OpenSubgroupGuard<'a>,
}

pub(super) async fn accept_streams(
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
                let Ok(receiver) = receiver else {
                    break;
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
    let mut ingest: Option<SubgroupIngest<'_>> = None;
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
            Ok(Some(DataObject::SubgroupHeader(received))) => {
                ingest = None;
                span.record("group_id", received.group_id);
                let subgroup_id = match received.subgroup_id {
                    moqt::SubgroupId::None => Some(0),
                    moqt::SubgroupId::Value(subgroup_id) => Some(subgroup_id),
                    moqt::SubgroupId::FirstObjectIdDelta => None,
                };
                let received = ReceivedHeader {
                    group_id: received.group_id,
                    publisher_priority: received.publisher_priority,
                    ends_group_on_fin: received.message_type.has_end_of_group(),
                    prev_object_id: None,
                };
                if let Some(subgroup_id) = subgroup_id {
                    ingest = Some(open_subgroup(
                        &cache,
                        &span,
                        received.with_subgroup_id(subgroup_id),
                    ));
                }
                header = Some(received);
            }
            Ok(Some(DataObject::SubgroupObject(field))) => {
                let Some(header) = header.as_mut() else {
                    span.record("end_reason", "object_before_header");
                    tracing::error!(%track_key, "subgroup object received before its header");
                    return;
                };
                let object_id = field.resolve_object_id(header.prev_object_id);
                header.prev_object_id = Some(object_id);
                let current = ingest.get_or_insert_with(|| {
                    open_subgroup(&cache, &span, header.with_subgroup_id(object_id))
                });
                let end_reason = match &field.subgroup_object {
                    moqt::SubgroupObject::Status { code, .. }
                        if *code == ObjectStatus::EndOfGroup as u64 =>
                    {
                        Some("end_of_group")
                    }
                    moqt::SubgroupObject::Status { code, .. }
                        if *code == ObjectStatus::EndOfTrack as u64 =>
                    {
                        Some("end_of_track")
                    }
                    _ => None,
                };
                let object =
                    match CachedObject::from_subgroup_object(&current.header, object_id, field) {
                        Ok(object) => object,
                        Err(error) => {
                            // draft-14 §10.2.1.1: an unknown Object Status is a
                            // protocol error; the upstream session is terminated.
                            span.record("end_reason", "protocol_violation");
                            tracing::error!(%track_key, %error, object_id, "invalid object status");
                            let _ = session_event_sender.send(
                                SessionEvent::protocol_violation_detected(
                                    publisher_session_id,
                                    format!("{error} on object {object_id}"),
                                ),
                            );
                            return;
                        }
                    };
                if current.open.insert(object).is_err() {
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
                        ingest.open.finish();
                    }
                    return;
                }
            }
            Ok(Some(DataObject::ObjectDatagram(_))) => {
                span.record("end_reason", "unexpected_datagram");
                tracing::error!(%track_key, "datagram received on a subgroup stream");
                return;
            }
            Ok(None) => {
                span.record("end_reason", "fin");
                if let (Some(header), Some(ingest)) = (&header, &ingest)
                    && header.ends_group_on_fin
                {
                    let end_of_group_id = header.prev_object_id.map_or(0, |id| id + 1);
                    if ingest
                        .open
                        .insert(CachedObject::end_of_group(&ingest.header, end_of_group_id))
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
                    ingest.open.finish();
                }
                tracing::debug!(%track_key, "stream finished");
                return;
            }
            Err(moqt::StreamReceiveError::Closed(error)) => {
                span.record("end_reason", "transport_closed");
                tracing::info!(%track_key, %error, "stream transport closed");
                return;
            }
            Err(moqt::StreamReceiveError::Decode(error)) => {
                span.record("end_reason", "decode_error");
                tracing::error!(%track_key, %error, "failed to decode stream data");
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
    header: SubgroupHeaderFields,
) -> SubgroupIngest<'a> {
    span.record("subgroup_id", header.subgroup_id);
    let open = cache.open_subgroup(header.key());
    SubgroupIngest { header, open }
}

impl ReceivedHeader {
    fn with_subgroup_id(&self, subgroup_id: u64) -> SubgroupHeaderFields {
        SubgroupHeaderFields {
            group_id: self.group_id,
            subgroup_id,
            publisher_priority: self.publisher_priority,
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use moqt::{ObjectStatus, SubgroupId};

    use super::*;
    use crate::modules::{
        data_plane::{
            cache::subgroup_key::SubgroupKey,
            cache::track_cache::NextObject,
            tests::harness::{
                PUBLISHER_SESSION_ID, RelayHarness, UpstreamSubgroupStream,
                fixtures::{
                    cached_object::stream_key,
                    data_object::{
                        make_header, make_header_with, make_payload_object, make_raw_status_object,
                        make_status_object,
                    },
                    location,
                },
            },
        },
        session::session_event::EventKind,
    };

    fn payload(object_id_delta: u64) -> DataObject {
        make_payload_object(object_id_delta, Bytes::from_static(b"payload"))
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
                make_status_object(0, ObjectStatus::EndOfGroup),
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
    async fn resolves_absolute_object_ids_from_deltas() {
        // Arrange: deltas 0, 0, 1 resolve to absolute ids 0, 1, 3
        let harness = RelayHarness::new();
        let mut upstream_stream = harness.open_upstream_stream();
        // Act
        send_all(
            &upstream_stream,
            vec![make_header(0), payload(0), payload(0), payload(1)],
        );
        upstream_stream.fin();
        upstream_stream.wait_reader_end().await;
        // Assert
        let ids: Vec<u64> = harness
            .cached_object_ids(stream_key(0))
            .await
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, vec![0, 1, 3]);
    }

    #[tokio::test]
    async fn end_of_group_header_type_synthesizes_status_object_on_fin() {
        // Arrange: Type 0x18 header (last object before FIN ends the group)
        let harness = RelayHarness::new();
        let mut upstream_stream = harness.open_upstream_stream();
        // Act
        send_all(
            &upstream_stream,
            vec![
                make_header_with(0, SubgroupId::None, true),
                payload(0),
                payload(0),
            ],
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
        send_all(&first_stream, vec![make_header(0), payload(0), payload(0)]);
        first_stream.fin();
        first_stream.wait_reader_end().await;
        let mut second_stream = harness.open_upstream_stream();
        // Act
        send_all(
            &second_stream,
            vec![make_header_with(0, SubgroupId::Value(1), true), payload(0)],
        );
        second_stream.fin();
        second_stream.wait_reader_end().await;
        // Assert
        harness.wait_track_malformed().await;
        let event = harness.expect_session_event().await;
        assert!(matches!(event.kind, EventKind::MalformedTrackDetected(_)));
    }

    #[tokio::test]
    async fn unknown_object_status_terminates_the_publisher_session() {
        // Arrange: status code 0x2 is not defined by draft-14 §10.2.1.1
        let mut harness = RelayHarness::new();
        let mut upstream_stream = harness.open_upstream_stream();
        // Act
        send_all(
            &upstream_stream,
            vec![make_header(0), payload(0), make_raw_status_object(0, 0x2)],
        );
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
            vec![make_header_with(0, SubgroupId::None, true), payload(0)],
        );
        upstream_stream.reset();
        upstream_stream.wait_reader_end().await;
        // Assert
        assert_eq!(
            harness.cached_object_ids(stream_key(0)).await,
            vec![(0, ObjectStatus::Normal)]
        );
    }

    #[tokio::test]
    async fn first_object_id_delta_header_takes_subgroup_id_from_first_object() {
        // Arrange: Type 0x12 header; the first object arrives with id 5
        let harness = RelayHarness::new();
        let mut subgroup_opened_receiver = harness.subscribe_subgroup_opened();
        let mut upstream_stream = harness.open_upstream_stream();
        // Act
        send_all(
            &upstream_stream,
            vec![
                make_header_with(0, SubgroupId::FirstObjectIdDelta, false),
                payload(5),
                payload(0),
            ],
        );
        upstream_stream.fin();
        upstream_stream.wait_reader_end().await;
        // Assert: the subgroup is opened, and announced, only once its id is known
        let key = SubgroupKey::Stream {
            group_id: 0,
            subgroup_id: 5,
        };
        assert_eq!(
            subgroup_opened_receiver.try_recv().map(|run| run.key),
            Ok(key)
        );
        let ids: Vec<u64> = harness
            .cached_object_ids(key)
            .await
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, vec![5, 6]);
    }
}
