use std::{sync::Arc, time::Duration};

use tokio::{task::JoinHandle, time::MissedTickBehavior};

use crate::modules::relay::cache::store::TrackCacheStore;

const DEFAULT_TTL_SECS: u64 = 30;
const DEFAULT_INTERVAL_SECS: u64 = 5;

fn duration_from_env(var: &str, default_secs: u64) -> Duration {
    let secs = std::env::var(var)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default_secs);
    Duration::from_secs(secs)
}

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
