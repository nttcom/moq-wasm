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
    ) -> anyhow::Result<()> {
        let group = cache.get_group(group_id).await;
        for object in group.read().await.iter() {
            sender.send_object((**object).clone()).await.inspect(|_| {
                tracing::debug!("Latest object sent successfully");
            })?;
        }

        Ok(())
    }

    pub(crate) async fn send_latest_object(
        &self,
        sender: &mut dyn DataSender,
        receiver: &mut tokio::sync::broadcast::Receiver<Arc<DataObject>>,
    ) -> anyhow::Result<()> {
        let object = receiver
            .recv()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to receive latest object: {}", e))?;
        sender.send_object((*object).clone()).await.inspect(|_| {
            tracing::debug!("Latest object sent successfully");
        })?;

        Ok(())
    }

    pub(crate) async fn send_absolute_start(
        &self,
        sender: &mut dyn DataSender,
        cache: &Arc<dyn Cache>,
        group_id: u64,
        object_id: u64,
        is_reverse: bool,
    ) -> anyhow::Result<()> {
        let group = cache.get_group(group_id).await;
        let frames = group.read().await;
        let mut start_flag = false;

        if is_reverse {
            for object in frames.iter().rev() {
                if !start_flag && object.object_id() <= Some(object_id) {
                    start_flag = true;
                }
                if start_flag {
                    sender.send_object((**object).clone()).await.inspect(|_| {
                        tracing::debug!("Latest object sent successfully");
                    })?;
                }
            }
        } else {
            for object in frames.iter() {
                if !start_flag && object.object_id() >= Some(object_id) {
                    start_flag = true;
                }
                if start_flag {
                    sender.send_object((**object).clone()).await.inspect(|_| {
                        tracing::debug!("Latest object sent successfully");
                    })?;
                }
            }
        };

        Ok(())
    }
}
