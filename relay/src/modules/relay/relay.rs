use std::sync::Arc;

use crate::modules::{
    core::{data_receiver::receiver::DataReceiver, data_sender::DataSender},
    enums::{FilterType, GroupOrder},
    relay::{
        caches::{cache::Cache, cache_map::CacheMap},
        object_sender::ObjectSender,
        relay_properties::RelayProperties,
    },
    types::{SessionId, compose_session_track_key},
};

pub(crate) struct Relay {
    pub(crate) relay_properties: RelayProperties,
}

impl Relay {
    pub(crate) fn init_object_receiver(
        &mut self,
        session_id: SessionId,
        track_alias: u64,
        group_id: Option<u64>,
        data_receiver: DataReceiver,
    ) {
        let track_key = compose_session_track_key(session_id, track_alias);
        self.initialize_if_needed(track_key);
        self.add_object_receiver(track_key, group_id, data_receiver);
    }

    pub(crate) fn add_object_receiver(
        &mut self,
        track_key: u128,
        group_id: Option<u64>,
        mut data_receiver: DataReceiver,
    ) {
        let queue = self.relay_properties.object_queue.clone();
        self.relay_properties.joinset.spawn(async move {
            tracing::info!("add object receiver");
            let mut current_group_id = group_id;
            while let Ok(data_object) = data_receiver.receive_object().await {
                tracing::debug!("receive object");
                let queue = queue.get_mut(&track_key);
                if queue.is_none() {
                    tracing::error!("Track key {} not found in object queue", track_key);
                    break;
                }
                let group_id = data_object.group_id().or(current_group_id);
                let Some(group_id) = group_id else {
                    tracing::warn!(track_key, "received object without group_id");
                    continue;
                };
                current_group_id = Some(group_id);
                queue
                    .unwrap()
                    .set_latest_object(track_key, group_id, data_object)
                    .await;
            }
            tracing::info!("object receiver task ended");
        });
    }

    pub(crate) fn init_object_sender(
        &mut self,
        session_id: SessionId,
        track_alias: u64,
        group_id: Option<u64>,
        datagram_sender: Box<dyn DataSender>,
        group_order: GroupOrder,
        filter_type: FilterType,
    ) {
        let track_key = compose_session_track_key(session_id, track_alias);
        self.initialize_if_needed(track_key);
        self.add_object_sender(
            track_key,
            group_id,
            datagram_sender,
            group_order,
            filter_type,
        );
    }

    pub(crate) fn add_object_sender(
        &mut self,
        track_key: u128,
        group_id: Option<u64>,
        datagram_sender: Box<dyn DataSender>,
        group_order: GroupOrder,
        filter_type: FilterType,
    ) {
        let cache = self
            .relay_properties
            .object_queue
            .get(&track_key)
            .unwrap()
            .clone();
        self.relay_properties.joinset.spawn(async move {
            tracing::info!("add object sender");
            let mut receiver = Some(cache.get_latest_receiver());
            let (mut start_group_id, start_object_id) = Self::get_start_ids(&cache, &filter_type);
            let mut object_sender = ObjectSender {
                sender: datagram_sender,
                cache,
                group_id: start_group_id,
            };
            loop {
                match filter_type {
                    FilterType::LatestObject | FilterType::LatestGroup => {
                        if let Some(ref mut recv) = receiver {
                            object_sender.send_latest_object(recv).await;
                        }
                    }
                    FilterType::AbsoluteStart { .. } => {
                        object_sender
                            .send_absolute_start(start_group_id, start_object_id)
                            .await;
                    }
                    FilterType::AbsoluteRange {
                        location: _,
                        end_group,
                    } => {
                        object_sender
                            .send_absolute_start(start_group_id, start_object_id)
                            .await;
                        if start_group_id < end_group {
                            break;
                        }
                    }
                }
                match group_order {
                    GroupOrder::Ascending | GroupOrder::Publisher => {
                        start_group_id += 1;
                    }
                    GroupOrder::Descending => {
                        start_group_id -= 1;
                    }
                };
            }
        });
    }

    fn initialize_if_needed(&mut self, track_key: u128) {
        self.relay_properties
            .object_queue
            .entry(track_key)
            .or_insert_with(|| Arc::new(CacheMap::new()));
    }

    fn get_start_ids(cache: &Arc<dyn Cache>, filter_type: &FilterType) -> (u64, u64) {
        match filter_type {
            FilterType::LatestGroup | FilterType::LatestObject => (cache.get_latest_group_id(), 0),
            FilterType::AbsoluteStart { location } => (location.group_id, location.object_id),
            FilterType::AbsoluteRange { location, .. } => (location.group_id, location.object_id),
        }
    }
}