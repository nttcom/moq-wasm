use std::sync::Arc;

use tokio::sync::RwLock;

use crate::modules::core::data_object::DataObject;

#[allow(dead_code)]
pub(crate) struct GroupCache {
    objects: RwLock<Vec<Arc<DataObject>>>,
    end_of_group: RwLock<bool>,
}

impl GroupCache {
    pub(crate) fn new() -> Self {
        Self {
            objects: RwLock::new(Vec::new()),
            end_of_group: RwLock::new(false),
        }
    }

    pub(crate) async fn append(&self, object: Arc<DataObject>) -> u64 {
        let mut objects = self.objects.write().await;
        objects.push(object);
        objects.len().saturating_sub(1) as u64
    }

    pub(crate) async fn get(&self, index: u64) -> Option<Arc<DataObject>> {
        let objects = self.objects.read().await;
        objects.get(index as usize).cloned()
    }

    pub(crate) async fn mark_end_of_group(&self) {
        let mut end_of_group = self.end_of_group.write().await;
        *end_of_group = true;
    }
}
