use std::{sync::Arc, time::Duration};

pub(crate) struct ThreadManager {
    checker_join_handle: tokio::task::JoinHandle<()>,
    join_handles: Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl ThreadManager {
    pub(super) fn new() -> Self {
        let join_handles = Arc::new(tokio::sync::Mutex::new(vec![]));
        let checker_join_handle = Self::create_checker(join_handles.clone());
        Self {
            checker_join_handle,
            join_handles,
        }
    }

    fn create_checker(
        join_handles: Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("thread checker")
            .spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(1));
                loop {
                    interval.tick().await;
                    if let Some(join_handle) =
                        Self::extract_first_finished_joinhandle(&join_handles).await
                    {
                        join_handle.await;
                    }
                }
            })
            .unwrap()
    }

    async fn extract_first_finished_joinhandle(
        join_handles: &Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let mut join_handles = join_handles.lock().await;
        join_handles
            .iter()
            .position(|j| j.is_finished())
            .map(|pos| join_handles.swap_remove(pos))
    }

    pub(crate) async fn add_join_handle(&mut self, join_handle: tokio::task::JoinHandle<()>) {
        self.join_handles.lock().await.push(join_handle);
    }
}

impl Drop for ThreadManager {
    fn drop(&mut self) {
        self.checker_join_handle.abort();
    }
}
