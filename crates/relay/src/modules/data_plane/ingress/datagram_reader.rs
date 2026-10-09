use tokio::sync::mpsc;

use crate::modules::{
    data_plane::{
        cache::subgroup_key::SubgroupKey,
        cache::{cached_object::CachedObject, track_cache::OpenSubgroupGuard},
        ingress::track_ingest_task::TrackIngest,
    },
    domain::{session_id::SessionId, track_key::TrackKey},
    session::{
        data_object::DataObject, data_receiver::datagram_receiver::DatagramReceiver,
        session_event::SessionEvent,
    },
};

pub(crate) async fn read_datagrams(ingest: TrackIngest, mut receiver: Box<dyn DatagramReceiver>) {
    let TrackIngest {
        track_key,
        publisher_session_id,
        cache,
        session_event_sender,
        mut stop_receiver,
    } = ingest;
    let mut current_group: Option<(u64, OpenSubgroupGuard<'_>)> = None;
    loop {
        let receive_result = tokio::select! {
            _ = stop_receiver.changed() => {
                tracing::info!(%track_key, "datagram reader stopped");
                break;
            }
            result = receiver.receive_object() => result,
        };

        match receive_result {
            Ok(object) => {
                let DataObject::ObjectDatagram(datagram) = object else {
                    tracing::error!(%track_key, "non-datagram object on datagram receiver");
                    continue;
                };
                let group_id = datagram.group_id;
                let open = match &mut current_group {
                    Some((current_group_id, open)) if *current_group_id == group_id => open,
                    slot => {
                        if let Some((_, previous)) = slot.take() {
                            previous.finish();
                        }
                        let key = SubgroupKey::Datagram { group_id };
                        let (_, open) = slot.insert((group_id, cache.open_subgroup(key)));
                        open
                    }
                };
                let object_id = datagram.field.resolve_object_id();
                if open
                    .insert(CachedObject::from_datagram(object_id, datagram))
                    .is_err()
                {
                    report_malformed_track(&session_event_sender, publisher_session_id, &track_key);
                    break;
                }
            }
            Err(error) if error.is::<moqt::MalformedTrackError>() => {
                cache.mark_malformed();
                report_malformed_track(&session_event_sender, publisher_session_id, &track_key);
                break;
            }
            Err(_) => {
                tracing::debug!(%track_key, "datagram receiver ended");
                break;
            }
        }
    }
}

fn report_malformed_track(
    session_event_sender: &mpsc::UnboundedSender<SessionEvent>,
    publisher_session_id: SessionId,
    track_key: &TrackKey,
) {
    tracing::warn!(%track_key, "malformed track detected; stopping datagram ingest");
    let _ = session_event_sender.send(SessionEvent::malformed_track_detected(
        publisher_session_id,
        track_key.clone(),
    ));
}

#[cfg(test)]
mod tests {
    use crate::modules::{
        session::session_event::EventKind,
        test_support::relay_harness::{FailingUpstream, RelayHarness},
    };

    #[tokio::test]
    async fn a_subgroup_stream_on_the_datagram_track_reports_the_malformed_track() {
        // Arrange
        let mut harness = RelayHarness::new();
        // Act
        harness
            .run_datagram_ingest(FailingUpstream(|| moqt::MalformedTrackError.into()))
            .await;
        // Assert
        harness.wait_track_malformed().await;
        let event = harness.expect_session_event().await;
        assert!(matches!(event.kind, EventKind::MalformedTrackDetected(_)));
    }
}
