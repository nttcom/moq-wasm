use dashmap::DashMap;
use uuid::Uuid;

pub(crate) struct ThreadManager {
    join_handles_map: DashMap<Uuid, tokio::task::JoinHandle<()>>,
}

impl ThreadManager {
    pub(super) fn new() -> Self {
        let join_handles_map = DashMap::new();
        Self { join_handles_map }
    }

    pub(crate) fn add(&mut self, uuid: Uuid, join_handle: tokio::task::JoinHandle<()>) {
        self.join_handles_map.insert(uuid, join_handle);
    }

    pub(crate) fn remove(&mut self, uuid: &Uuid) {
        if let Some(join_handle) = self.join_handles_map.remove(uuid) {
            join_handle.1.abort();
        }
    }
}

impl Drop for ThreadManager {
    fn drop(&mut self) {
        for join_handle in self.join_handles_map.iter() {
            join_handle.value().abort();
        }
    }
}
