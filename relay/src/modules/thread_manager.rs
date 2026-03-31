use dashmap::DashMap;

use crate::modules::types::SessionId;

pub(crate) struct ThreadManager {
    join_handles_map: DashMap<SessionId, tokio::task::JoinHandle<()>>,
}

impl ThreadManager {
    pub(super) fn new() -> Self {
        let join_handles_map = DashMap::new();
        Self { join_handles_map }
    }

    pub(crate) fn add(&mut self, session_id: SessionId, join_handle: tokio::task::JoinHandle<()>) {
        self.join_handles_map.insert(session_id, join_handle);
    }

    pub(crate) fn remove(&mut self, session_id: &SessionId) {
        if let Some((_removed_session_id, join_handle)) = self.join_handles_map.remove(session_id) {
            join_handle.abort();
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
