use crate::modules::{
    data_plane::{
        cache::subgroup_key::SubgroupKey,
        cache::{cached_object::CachedObject, track_cache::OpenSubgroupGuard},
        ingress::track_ingest_task::TrackIngest,
    },
    session::{
        data_object::DataObject, data_receiver::datagram_receiver::DatagramReceiver,
        session_event::SessionEvent,
    },
};

pub(super) async fn read_datagrams(ingest: TrackIngest, mut receiver: Box<dyn DatagramReceiver>) {
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
                    tracing::warn!(
                        %track_key,
                        group_id,
                        "malformed track detected; stopping datagram ingest"
                    );
                    let _ = session_event_sender.send(SessionEvent::malformed_track_detected(
                        publisher_session_id,
                        track_key.clone(),
                    ));
                    break;
                }
            }
            Err(_) => {
                tracing::debug!(%track_key, "datagram receiver ended");
                break;
            }
        }
    }
}
