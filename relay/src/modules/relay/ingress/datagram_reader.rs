use std::{collections::HashMap, sync::Arc};

use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

use crate::modules::{
    core::{data_object::DataObject, data_receiver::datagram_receiver::DatagramReceiver},
    relay::{
        cache::{cached_object::CachedObject, store::TrackCacheStore, track_cache::LiveSubgroup},
        notifications::{track_event::TrackEvent, track_notifier::ObjectNotifyProducerMap},
        types::SubgroupKey,
    },
    session_event::SessionEvent,
    types::{SessionId, TrackKey},
};

pub(crate) struct DatagramReceiveStart {
    pub(crate) track_key: TrackKey,
    pub(crate) publisher_session_id: SessionId,
    pub(crate) receiver: Box<dyn DatagramReceiver>,
}

pub(crate) enum DatagramReceiveCommand {
    Start(DatagramReceiveStart),
    Stop {
        track_key: TrackKey,
        publisher_session_id: SessionId,
    },
}

pub(crate) struct DatagramReader {
    join_handle: JoinHandle<()>,
}

impl DatagramReader {
    pub(crate) fn run(
        mut receiver: mpsc::Receiver<DatagramReceiveCommand>,
        cache_store: Arc<TrackCacheStore>,
        object_notify_producer_map: Arc<ObjectNotifyProducerMap>,
        session_event_sender: mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut joinset = tokio::task::JoinSet::new();
            let mut stop_senders = HashMap::<TrackKey, (watch::Sender<bool>, SessionId)>::new();
            loop {
                tokio::select! {
                    Some(command) = receiver.recv() => {
                        match command {
                            DatagramReceiveCommand::Start(cmd) => {
                                let DatagramReceiveStart { track_key, publisher_session_id, receiver } = cmd;
                                // draft-14 §8.2 Multiple Publishers: for now keep the first publisher and
                                // ignore later ones. FIXME: GOAWAY migration needs ingesting from multiple
                                // publishers with per-object dedup (SHOULD); first-writer-wins is a stopgap.
                                if stop_senders.contains_key(&track_key) {
                                    tracing::warn!(%track_key, "ignoring additional publisher for active track");
                                    continue;
                                }

                                let (stop_sender, stop_receiver) = watch::channel(false);
                                stop_senders.insert(track_key.clone(), (stop_sender, publisher_session_id));

                                let cache_store = cache_store.clone();
                                let sender_map = object_notify_producer_map.clone();
                                let session_event_sender = session_event_sender.clone();
                                joinset.spawn(async move {
                                    Self::read_loop(
                                        track_key.clone(),
                                        publisher_session_id,
                                        receiver,
                                        stop_receiver,
                                        cache_store,
                                        sender_map,
                                        session_event_sender,
                                    )
                                    .await;
                                    track_key
                                });
                            }
                            DatagramReceiveCommand::Stop { track_key, publisher_session_id } => {
                                // Only the owning publisher may stop the reader, so a different
                                // publisher of the same track leaving does not tear down the active one.
                                if stop_senders.get(&track_key).is_some_and(|(_, owner)| *owner == publisher_session_id)
                                    && let Some((stop_sender, _)) = stop_senders.remove(&track_key)
                                {
                                    let _ = stop_sender.send(true);
                                    tracing::info!(%track_key, "datagram ingress track stop requested");
                                }
                            }
                        }
                    }
                    Some(result) = joinset.join_next() => {
                        match result {
                            Ok(track_key) => {
                                stop_senders.remove(&track_key);
                                tracing::debug!(%track_key, "datagram ingress track ended");
                            }
                            Err(e) => {
                                tracing::error!("datagram read task panicked: {:?}", e);
                            }
                        }
                    }
                    else => break,
                }
            }
        });
        Self { join_handle }
    }

    async fn read_loop(
        track_key: TrackKey,
        publisher_session_id: SessionId,
        mut receiver: Box<dyn DatagramReceiver>,
        mut stop_receiver: watch::Receiver<bool>,
        cache_store: Arc<TrackCacheStore>,
        object_notify_producer_map: Arc<ObjectNotifyProducerMap>,
        session_event_sender: mpsc::UnboundedSender<SessionEvent>,
    ) {
        let cache = cache_store.get_or_create(&track_key);
        cache.begin_live_ingest();
        let notify = object_notify_producer_map.get_or_create(&track_key);
        let mut current_group: Option<(u64, LiveSubgroup<'_>)> = None;
        let mut prev_object_id: Option<u64> = None;
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
                    let live = match &mut current_group {
                        Some((current_group_id, live)) if *current_group_id == group_id => live,
                        slot => {
                            let key = SubgroupKey::Datagram { group_id };
                            let (_, live) = slot.insert((group_id, cache.open_live_subgroup(key)));
                            prev_object_id = None;
                            let _ = notify.send(TrackEvent::SubgroupOpened(key));
                            live
                        }
                    };
                    let object_id = datagram.field.resolve_object_id(prev_object_id);
                    prev_object_id = Some(object_id);
                    if live
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
        drop(current_group);
        cache.end_live_ingest();
    }
}

impl Drop for DatagramReader {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
