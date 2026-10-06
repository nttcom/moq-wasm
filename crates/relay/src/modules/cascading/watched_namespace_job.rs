use std::{sync::Arc, time::Duration};

use tokio::{task::JoinHandle, time::MissedTickBehavior};

use crate::modules::{
    cascading::watched_namespace_routes::WatchedNamespaceRoutes,
    domain::pub_sub_directory::InMemoryLocalPubSubDirectory,
};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) struct WatchedNamespaceJob {
    join_handle: JoinHandle<()>,
}

impl WatchedNamespaceJob {
    pub(crate) fn run(
        routes: Arc<WatchedNamespaceRoutes>,
        table: Arc<InMemoryLocalPubSubDirectory>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(RECONCILE_INTERVAL);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                routes.reconcile(&table.watched_namespaces()).await;
            }
        });
        Self { join_handle }
    }
}

impl Drop for WatchedNamespaceJob {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
