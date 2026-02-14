use std::sync::Arc;

use crate::modules::core::data_object::DataObject;

pub(crate) struct LatestInfo {
    pub(crate) group_id: u64,
    latest_object: tokio::sync::watch::Sender<Option<Arc<DataObject>>>,
}

impl LatestInfo {
    pub(crate) fn new() -> Self {
        let (sender, _receiver) = tokio::sync::watch::channel(None);
        Self {
            group_id: 0,
            latest_object: sender,
        }
    }

    pub(crate) fn get_receiver(&self) -> tokio::sync::watch::Receiver<Option<Arc<DataObject>>> {
        self.latest_object.subscribe()
    }

    pub(crate) fn set_latest_object(&self, object: Arc<DataObject>) {
        let _ = self.latest_object.send(Some(object));
    }
}
