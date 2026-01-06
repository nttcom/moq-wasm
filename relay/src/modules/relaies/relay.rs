use std::sync::Arc;

use crate::modules::{
    core::{data_object::DataObject, data_receiver::DataReceiver, data_sender::DataSender},
    enums::{FilterType, GroupOrder},
    relaies::{
        caches::cache_map::CacheMap, object_sender::ObjectSender, relay_properties::RelayProperties,
    },
};

pub(crate) struct Relay {
    pub(crate) relay_properties: RelayProperties,
}

impl Relay {
    pub(crate) fn add_object_receiver(&mut self, mut data_receiver: Box<dyn DataReceiver>) {
        let track_alias = data_receiver.track_alias();
        self.initialize_if_needed(track_alias);
        let queue = self.relay_properties.object_queue.clone();
        self.relay_properties.joinset.spawn(async move {
            tracing::info!("add object receiver");
            while let Ok(data_object) = data_receiver.receive_object().await {
                tracing::info!("receive object");
                let data_object = DataObject {
                    moqt_data_object: data_object,
                };
                let queue = queue.get_mut(&track_alias);
                if queue.is_none() {
                    tracing::error!("Track alias {} not found in object queue", track_alias);
                    break;
                }
                queue.unwrap().set_latest_object(data_object).await;
            }
        });
    }

    pub(crate) fn add_object_sender(
        &mut self,
        track_alias: u64,
        datagram_sender: Box<dyn DataSender>,
        group_order: GroupOrder,
        filter_type: FilterType,
    ) {
        self.initialize_if_needed(track_alias);
        let cache = self
            .relay_properties
            .object_queue
            .get(&track_alias)
            .unwrap()
            .clone();
        self.relay_properties.joinset.spawn(async move {
            tracing::info!("add object sender");
            let object_sender = ObjectSender {};
            let mut receiver = Some(cache.get_latest_receiver());
            let (mut start_group_id, start_object_id) = match filter_type {
                FilterType::LatestGroup => (cache.get_latest_group_id(), 0u64),
                FilterType::AbsoluteStart { ref location } => {
                    (location.group_id, location.object_id)
                }
                FilterType::AbsoluteRange { ref location, .. } => {
                    (location.group_id, location.object_id)
                }
                _ => (0u64, 0u64),
            };
            loop {
                match filter_type {
                    FilterType::LatestGroup => {
                        object_sender
                            .send_latest_group(datagram_sender.as_ref(), &cache, start_group_id)
                            .await;
                    }
                    FilterType::LatestObject => {
                        if let Some(ref mut recv) = receiver {
                            object_sender
                                .send_latest_object(datagram_sender.as_ref(), recv)
                                .await;
                        }
                    }
                    FilterType::AbsoluteStart { .. } => {
                        object_sender
                            .send_absolute_start(
                                datagram_sender.as_ref(),
                                &cache,
                                start_group_id,
                                start_object_id,
                                group_order == GroupOrder::Descending,
                            )
                            .await;
                    }
                    FilterType::AbsoluteRange {
                        location: _,
                        end_group,
                    } => {
                        object_sender
                            .send_absolute_start(
                                datagram_sender.as_ref(),
                                &cache,
                                start_group_id,
                                start_object_id,
                                group_order == GroupOrder::Descending,
                            )
                            .await;
                        if start_group_id == end_group {
                            break;
                        }
                    }
                }
                match group_order {
                    GroupOrder::Ascending => {
                        start_group_id += 1;
                    }
                    GroupOrder::Descending => {
                        start_group_id -= 1;
                    }
                    GroupOrder::Publisher => {
                        // No change in group_id for Publisher order
                    }
                };
            }
        });
    }

    fn initialize_if_needed(&mut self, track_alias: u64) {
        self.relay_properties
            .object_queue
            .entry(track_alias)
            .or_insert_with(|| Arc::new(CacheMap::new()));
    }
}
