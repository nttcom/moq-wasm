use std::sync::Arc;

use crate::modules::{
    core::data_object::DataObject, enums::Location, relay::caches::latest_info::LatestInfo,
};

#[async_trait::async_trait]
pub(crate) trait Cache: Send + Sync + 'static {
    async fn add_object(&self, track_key: u128, group_id: u64, object: DataObject) -> u64;
    async fn get_group(&self, group_id: u64) -> Arc<tokio::sync::RwLock<Vec<Arc<DataObject>>>>;
}
