use std::sync::Arc;

use tokio::{task::JoinHandle, time::MissedTickBehavior};

use crate::modules::relay::cache::{duration::duration_from_env, store::TrackCacheStore};

const DEFAULT_TTL_SECS: u64 = 30;
const DEFAULT_INTERVAL_SECS: u64 = 5;

pub(crate) fn spawn_cache_eviction_job(cache_store: Arc<TrackCacheStore>) -> JoinHandle<()> {
    let ttl = duration_from_env("RELAY_CACHE_TTL_SECS", DEFAULT_TTL_SECS);
    let interval = duration_from_env("RELAY_CACHE_EVICT_INTERVAL_SECS", DEFAULT_INTERVAL_SECS);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            cache_store.evict(ttl).await;
        }
    })
}
