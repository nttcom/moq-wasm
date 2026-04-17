use std::sync::Arc;

use crate::modules::{
    core::{data_object::DataObject, data_sender::DataSender},
    relaies::caches::cache::Cache,
};

pub(crate) struct ObjectSender;

impl ObjectSender {
    pub(crate) async fn send_latest_group(
        &self,
        sender: &mut dyn DataSender,
        cache: &Arc<dyn Cache>,
        group_id: u64,
    ) -> bool {
        let group = cache.get_group(group_id).await;
        for object in group.read().await.iter() {
            match sender.send_object((**object).clone()).await {
                Ok(_) => {
                    tracing::debug!("Latest object sent successfully");
                }
                Err(e) => {
                    tracing::warn!("Failed to send latest object: {}", e);
                    return false;
                }
            }
        }

        true
    }

    pub(crate) async fn send_latest_object(
        &self,
        sender: &mut dyn DataSender,
        receiver: &mut tokio::sync::broadcast::Receiver<Arc<DataObject>>,
    ) -> bool {
        let object = receiver.recv().await;
        if let Ok(object) = object {
            match sender.send_object((*object).clone()).await {
                Ok(_) => {
                    tracing::debug!("Latest object sent successfully");
                }
                Err(e) => {
                    tracing::warn!("Failed to send latest object: {}", e);
                    return false;
                }
            }
        } else {
            tracing::warn!("Failed to receive latest object");
            return false;
        }

        true
    }

    pub(crate) async fn send_absolute_start(
        &self,
        sender: &mut dyn DataSender,
        cache: &Arc<dyn Cache>,
        group_id: u64,
        object_id: u64,
        is_reverse: bool,
    ) -> bool {
        let group = cache.get_group(group_id).await;
        let frames = group.read().await;
        let mut start_flag = false;

        if is_reverse {
            for object in frames.iter().rev() {
                if !start_flag && object.object_id() <= Some(object_id) {
                    start_flag = true;
                }
                if start_flag {
                    match sender.send_object((**object).clone()).await {
                        Ok(_) => {
                            tracing::debug!("Latest object sent successfully");
                        }
                        Err(e) => {
                            tracing::warn!("Failed to send latest object: {}", e);
                            return false;
                        }
                    }
                }
            }
        } else {
            for object in frames.iter() {
                if !start_flag && object.object_id() >= Some(object_id) {
                    start_flag = true;
                }
                if start_flag {
                    match sender.send_object((**object).clone()).await {
                        Ok(_) => {
                            tracing::debug!("Latest object sent successfully");
                        }
                        Err(e) => {
                            tracing::warn!("Failed to send latest object: {}", e);
                            return false;
                        }
                    }
                }
            }
        };

        true
    }
}
