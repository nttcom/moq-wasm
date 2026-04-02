use std::sync::Arc;

use crate::modules::{
    core::data_sender::DataSender,
    relay::notifications::{cache::Cache, latest_info::LatestInfo},
};

pub(crate) struct ObjectSender {
    pub(crate) sender: Box<dyn DataSender>,
    pub(crate) cache: Arc<dyn Cache>,
    pub(crate) group_id: u64,
}

impl ObjectSender {
    pub(crate) async fn send_latest_object(
        &mut self,
        receiver: &mut tokio::sync::broadcast::Receiver<LatestInfo>,
    ) {
        let info = match receiver.recv().await {
            Ok(info) => info,
            Err(e) => {
                tracing::warn!("Failed to receive latest object info: {}", e);
                return;
            }
        };
        match info {
            LatestInfo::LatestObject {
                group_id,
                offset,
            } => {
                if group_id != self.group_id {
                    tracing::error!(
                        "Received latest object info for group_id {}, but expected {}",
                        group_id,
                        self.group_id
                    );
                    return;
                }
                tracing::debug!(
                    "Received latest object info: group_id={}, offset={}",
                    group_id,
                    offset
                );
                let data = self.cache.get_group(group_id).await;
                if let Some(object) = data.read().await.get(offset as usize) {
                    match self.sender.send_object((**object).clone()).await {
                        Ok(_) => {
                            tracing::debug!("Latest object sent successfully");
                        }
                        Err(e) => {
                            tracing::warn!("Failed to send latest object: {}", e);
                        }
                    }
                } else {
                    tracing::warn!("No object found at offset {} in group {}", offset, group_id);
                }
            }
            LatestInfo::EndOfGroup { group_id } => {
                tracing::info!("Received end of group info: group_id={}", group_id);
            }
        }
    }

    pub(crate) async fn send_absolute_start(&mut self, group_id: u64, offset: u64) {
        let group = self.cache.get_group(group_id).await;
        let frames = group.read().await;
        if let Some(object) = frames.get(offset as usize) {
            match self.sender.send_object((**object).clone()).await {
                Ok(_) => {
                    tracing::debug!("Object sent successfully");
                }
                Err(e) => {
                    tracing::warn!("Failed to send object: {}", e);
                }
            }
        } else {
            tracing::warn!("No object found at offset {} in group {}", offset, group_id);
        }
    }
}