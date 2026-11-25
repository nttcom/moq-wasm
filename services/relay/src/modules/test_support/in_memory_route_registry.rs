use std::{
    collections::{BTreeSet, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use tokio::sync::Notify;

use crate::modules::{
    cascading::route_registry::{RegisterRouteError, RelayInfo, RelayRouteRegistry},
    domain::track_key::TrackKey,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct Routes {
    pub(crate) namespace_publishers: BTreeSet<String>,
    pub(crate) namespace_subscribers: BTreeSet<String>,
    pub(crate) track_publishers: HashSet<TrackKey>,
}

/// A route registry that keeps what the relay registers, as Redis does, and
/// lists no other relay.
#[derive(Default)]
pub(crate) struct InMemoryRouteRegistry {
    routes: Mutex<Routes>,
    held_call: Mutex<Option<Arc<Notify>>>,
    fail_next_registration: AtomicBool,
}

impl InMemoryRouteRegistry {
    pub(crate) fn routes(&self) -> Routes {
        self.lock().clone()
    }

    /// The next call that registers or unregisters a namespace route waits
    /// for `notify_one` on the returned `Notify` before it takes effect.
    pub(crate) fn hold_next_namespace_call(&self) -> Arc<Notify> {
        let released = Arc::new(Notify::new());
        *self.held_call.lock().unwrap() = Some(released.clone());
        released
    }

    pub(crate) fn fail_next_namespace_registration(&self) {
        self.fail_next_registration.store(true, Ordering::SeqCst);
    }

    fn registration_fails(&self) -> bool {
        self.fail_next_registration.swap(false, Ordering::SeqCst)
    }

    async fn pass_held_call(&self) {
        let held = self.held_call.lock().unwrap().take();
        if let Some(released) = held {
            released.notified().await;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Routes> {
        self.routes.lock().unwrap()
    }
}

#[async_trait]
impl RelayRouteRegistry for InMemoryRouteRegistry {
    async fn mark_draining(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn find_migration_target(&self) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    async fn register_namespace_publisher(&self, track_namespace: &str) -> anyhow::Result<()> {
        self.pass_held_call().await;
        if self.registration_fails() {
            anyhow::bail!("route registry down");
        }
        self.lock()
            .namespace_publishers
            .insert(track_namespace.to_string());
        Ok(())
    }

    /// Like `register_namespace_subscriber.lua`, refuses a prefix the relay
    /// already holds.
    async fn register_namespace_subscriber(
        &self,
        track_namespace_prefix: &str,
    ) -> Result<(), RegisterRouteError> {
        self.pass_held_call().await;
        if self
            .lock()
            .namespace_subscribers
            .insert(track_namespace_prefix.to_string())
        {
            Ok(())
        } else {
            Err(RegisterRouteError::Conflict)
        }
    }

    async fn find_active_namespace_publishers(
        &self,
        _track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayInfo>> {
        Ok(Vec::new())
    }

    async fn register_track_publisher(&self, track_key: &TrackKey) -> anyhow::Result<()> {
        self.lock().track_publishers.insert(track_key.clone());
        Ok(())
    }

    async fn unregister_track_publisher(&self, track_key: &TrackKey) -> anyhow::Result<()> {
        self.lock().track_publishers.remove(track_key);
        Ok(())
    }

    async fn find_active_track_publishers(
        &self,
        _track_key: &TrackKey,
    ) -> anyhow::Result<Vec<RelayInfo>> {
        Ok(Vec::new())
    }

    async fn register_watched_namespace(&self, _track_namespace: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn unregister_watched_namespace(&self, _track_namespace: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn find_namespace_publishers_by_prefix(
        &self,
        _track_namespace_prefix: &str,
    ) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn unregister_namespace_publisher(&self, track_namespace: &str) -> anyhow::Result<()> {
        self.pass_held_call().await;
        self.lock().namespace_publishers.remove(track_namespace);
        Ok(())
    }

    async fn unregister_namespace_subscriber(
        &self,
        track_namespace_prefix: &str,
    ) -> anyhow::Result<()> {
        self.pass_held_call().await;
        self.lock()
            .namespace_subscribers
            .remove(track_namespace_prefix);
        Ok(())
    }

    async fn find_namespace_subscribers(
        &self,
        _track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayInfo>> {
        Ok(Vec::new())
    }
}
