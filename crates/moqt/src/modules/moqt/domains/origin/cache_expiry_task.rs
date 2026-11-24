use std::{sync::Weak, time::Duration};

use tokio::sync::Mutex;

use super::{OriginState, TrackKey};
use crate::modules::executor::{self, AbortOnDrop};

/// Drops a track's cached objects as they expire, so a track nobody writes
/// to holds none past its retention. Ends once the cache is empty.
pub(super) struct CacheExpiryTask {
    _handle: AbortOnDrop,
}

impl CacheExpiryTask {
    pub(super) fn run(
        state: Weak<Mutex<OriginState>>,
        key: TrackKey,
        expires_at: Duration,
    ) -> Self {
        let handle = executor::spawn("Origin Cache Expiry", async move {
            let mut expires_at = expires_at;
            loop {
                executor::sleep(expires_at.saturating_sub(executor::now())).await;
                let Some(state) = state.upgrade() else {
                    return;
                };
                let mut state = state.lock().await;
                let Some(track) = state.tracks.get_mut(&key) else {
                    return;
                };
                track.cache.evict_expired(executor::now());
                match track.cache.next_expiry() {
                    Some(next) => expires_at = next,
                    None => {
                        // Dropping the slot aborts this task; the abort takes effect only at an await, and none follows.
                        track.cache_expiry = None;
                        return;
                    }
                }
            }
        })
        .abort_on_drop();
        Self { _handle: handle }
    }
}
