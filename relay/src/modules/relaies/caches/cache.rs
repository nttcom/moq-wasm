use std::sync::Arc;

use crate::modules::core::data_object::DataObject;

#[async_trait::async_trait]
pub(crate) trait Cache: Send + Sync + 'static {
    fn get_latest_group_id(&self) -> u64;
    fn get_latest_receiver(&self) -> tokio::sync::watch::Receiver<Option<Arc<DataObject>>>;
    async fn set_latest_object(&self, object: DataObject);
    async fn get_group(&self, group_id: u64) -> Arc<tokio::sync::RwLock<Vec<Arc<DataObject>>>>;
}
