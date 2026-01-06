use std::sync::{Arc, OnceLock};

use crate::modules::core::data_object::DataObject;

pub(crate) struct LatestInfo {
    pub(crate) group_id: u64,
    latest_object: OnceLock<tokio::sync::watch::Sender<Arc<DataObject>>>,
}

impl LatestInfo {
    pub(crate) fn new() -> Self {
        Self {
            group_id: 0,
            latest_object: OnceLock::new(),
        }
    }

    pub(crate) fn get_receiver(&self) -> tokio::sync::watch::Receiver<Arc<DataObject>> {
        self.latest_object.get().unwrap().subscribe()
    }

    pub(crate) fn set_latest_object(&self, object: Arc<DataObject>) {
        self.latest_object.get_or_init(|| {
            let (sender, _receiver) = tokio::sync::watch::channel(object.clone());
            sender
        });
        let _ = self.latest_object.get().unwrap().send(object);
    }
}
