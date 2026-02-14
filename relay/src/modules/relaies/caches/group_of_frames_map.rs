use std::sync::Arc;

use crate::modules::core::data_object::DataObject;

pub(crate) struct GroupOfFramesMap {
    pub(crate) frames: Arc<tokio::sync::RwLock<Vec<Arc<DataObject>>>>,
}

impl GroupOfFramesMap {
    pub(crate) fn new() -> Self {
        Self {
            frames: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        }
    }
}
