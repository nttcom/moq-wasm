use std::sync::Arc;

use tokio::sync::{Notify, RwLock};

use crate::modules::core::data_object::DataObject;

pub(crate) struct GroupCache {
    objects: RwLock<Vec<Arc<DataObject>>>,
    end_of_group: RwLock<bool>,
    notify: Notify,
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
        objects.push(object);
        let index = objects.len().saturating_sub(1) as u64;
        drop(objects);
        self.notify.notify_waiters();
        index
    }

    pub(crate) async fn get(&self, index: u64) -> Option<Arc<DataObject>> {
        let objects = self.objects.read().await;
        objects.get(index as usize).cloned()
    }

    /// index のオブジェクトが存在しない場合、append または mark_end_of_group が来るまで待機する。
    /// グループがクローズされていれば None を返す。
    pub(crate) async fn get_or_wait(&self, index: u64) -> Option<Arc<DataObject>> {
        loop {
            // notified() は get() より先に作成してレースを防ぐ
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
}
