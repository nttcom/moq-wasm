use std::sync::Arc;

use tokio::sync::{Notify, RwLock};

use crate::modules::core::data_object::DataObject;

#[derive(Clone)]
pub(crate) struct CachedObject {
    pub(crate) object_id: Option<u64>,
    pub(crate) data: Arc<DataObject>,
}

pub(crate) struct GroupCache {
    objects: RwLock<Vec<CachedObject>>,
    end_of_group: RwLock<bool>,
    notify: Notify,
}

fn resolve_absolute_object_id(object: &DataObject, prev_object_id: Option<u64>) -> Option<u64> {
    match object {
        DataObject::SubgroupHeader(_) => None,
        DataObject::SubgroupObject(field) => Some(field.resolve_object_id(prev_object_id)),
        DataObject::ObjectDatagram(datagram) => datagram.field.object_id(),
    }
}

impl GroupCache {
    pub(crate) fn new() -> Self {
        Self {
            objects: RwLock::new(Vec::new()),
            end_of_group: RwLock::new(false),
            notify: Notify::new(),
        }
    }

    pub(crate) async fn append(&self, object: Arc<DataObject>) -> u64 {
        let mut objects = self.objects.write().await;
        let prev_object_id = objects.last().and_then(|o| o.object_id);
        let object_id = resolve_absolute_object_id(&object, prev_object_id);
        objects.push(CachedObject {
            object_id,
            data: object,
        });
        let index = objects.len().saturating_sub(1) as u64;
        drop(objects);
        self.notify.notify_waiters();
        index
    }

    pub(crate) async fn get(&self, index: u64) -> Option<Arc<DataObject>> {
        let objects = self.objects.read().await;
        objects.get(index as usize).map(|o| o.data.clone())
    }

    pub(crate) async fn last_object_id(&self) -> Option<u64> {
        self.objects.read().await.last().and_then(|o| o.object_id)
    }

    /// Waits until the object at `index` is available, or returns `None` if the group is closed.
    pub(crate) async fn get_or_wait(&self, index: u64) -> Option<Arc<DataObject>> {
        loop {
            // Create notified() before get() to avoid a race between the check and the wait.
            let notified = self.notify.notified();
            if let Some(obj) = self.get(index).await {
                return Some(obj);
            }
            if self.is_closed().await {
                return None;
            }
            notified.await;
        }
    }

    pub(crate) async fn mark_end_of_group(&self) {
        let mut end_of_group = self.end_of_group.write().await;
        *end_of_group = true;
        drop(end_of_group);
        self.notify.notify_waiters();
    }

    pub(crate) async fn is_closed(&self) -> bool {
        *self.end_of_group.read().await
    }

    pub(crate) async fn snapshot(&self) -> Vec<CachedObject> {
        self.objects.read().await.clone()
    }
}
