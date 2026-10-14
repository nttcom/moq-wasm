use std::sync::Arc;

use tokio::{task::JoinHandle, time::MissedTickBehavior};

use crate::{config::CacheConfig, modules::data_plane::cache::store::TrackCacheStore};

pub(crate) fn spawn_cache_eviction_job(
    cache_store: Arc<TrackCacheStore>,
    config: CacheConfig,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(config.evict_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            cache_store.evict(config.ttl);
        }
    })
}
