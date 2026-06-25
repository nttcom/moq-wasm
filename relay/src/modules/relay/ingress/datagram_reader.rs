use std::{collections::HashMap, sync::Arc};

use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

use crate::modules::{
    core::data_receiver::datagram_receiver::DatagramReceiver,
    relay::{
        cache::store::TrackCacheStore,
        notifications::{track_event::TrackEvent, track_notifier::ObjectNotifyProducerMap},
    },
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
                                joinset.spawn(async move {
                                    Self::read_loop(
                                        track_key.clone(),
                                        receiver,
                                        stop_receiver,
                                        cache_store,
                                        sender_map,
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
        mut receiver: Box<dyn DatagramReceiver>,
        mut stop_receiver: watch::Receiver<bool>,
        cache_store: Arc<TrackCacheStore>,
        object_notify_producer_map: Arc<ObjectNotifyProducerMap>,
    ) {
        let mut current_group_id: Option<u64> = None;
        let cache = cache_store.get_or_create(&track_key);
        let notify = object_notify_producer_map.get_or_create(&track_key);
        loop {
            let receive_result = tokio::select! {
                _ = stop_receiver.changed() => {
                    if let Some(group_id) = current_group_id {
                        cache.close_datagram_group(group_id).await;
                    }
                    tracing::info!(%track_key, "datagram reader stopped");
                    return;
                }
                result = receiver.receive_object() => result,
            };

            match receive_result {
                Ok(object) => {
                    let group_id = object.group_id().or(current_group_id).unwrap_or(0);
                    if current_group_id != Some(group_id) {
                        if let Some(old_group) = current_group_id {
                            cache.close_datagram_group(old_group).await;
                        }
                        current_group_id = Some(group_id);
                        let _ = notify.send(TrackEvent::DatagramOpened { group_id });
                    }
                    let object_id = object.resolve_absolute_object_id(None);
                    cache
                        .append_datagram_object(group_id, object_id, object)
                        .await;
                }
                Err(_) => {
                    // Ensure the last group is closed before exiting.
                    if let Some(group_id) = current_group_id {
                        cache.close_datagram_group(group_id).await;
                    }
                    tracing::debug!(%track_key, "datagram receiver ended");
                    return;
                }
            }
        }
    }
}

impl Drop for DatagramReader {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
