use std::sync::Arc;

use crate::modules::{
    core::data_object::DataObject, enums::Location, relay::caches::latest_info::LatestInfo,
};

#[async_trait::async_trait]
pub(crate) trait Cache: Send + Sync + 'static {
    fn get_latest_group_id(&self) -> u64;
    fn get_latest_receiver(&self) -> tokio::sync::broadcast::Receiver<LatestInfo>;
    async fn set_latest_object(&self, track_key: u128, group_id: u64, object: DataObject);
    async fn notify_end_of_group(&self, track_key: u128, group_id: u64);
    async fn get_group(&self, group_id: u64) -> Arc<tokio::sync::RwLock<Vec<Arc<DataObject>>>>;
    async fn get_offset(&self, location: Location) -> Option<u64>;
}