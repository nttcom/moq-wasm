use std::{collections::HashMap, net::IpAddr, time::Duration};

use tokio::time::Instant;

/// Reports a client address may send in a burst.
const BURST: f64 = 10.0;
/// Reports a client address regains per second.
const REFILL_PER_SECOND: f64 = 1.0;
/// How often buckets that have refilled are forgotten.
const PRUNE_INTERVAL: Duration = Duration::from_secs(60);

/// A token bucket per client address.
pub(super) struct RateLimiter {
    state: std::sync::Mutex<State>,
}

struct State {
    buckets: HashMap<IpAddr, Bucket>,
    last_prune: Instant,
}

struct Bucket {
    tokens: f64,
    refilled_at: Instant,
}

impl Bucket {
    fn refill(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.refilled_at).as_secs_f64();
        self.tokens = (self.tokens + elapsed * REFILL_PER_SECOND).min(BURST);
        self.refilled_at = now;
    }
}

impl RateLimiter {
    pub(super) fn new() -> Self {
        Self {
            state: std::sync::Mutex::new(State {
                buckets: HashMap::new(),
                last_prune: Instant::now(),
            }),
        }
    }

    /// Takes one report from `address`'s bucket; `false` when it is empty.
    pub(super) fn allow(&self, address: IpAddr) -> bool {
        let now = Instant::now();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if now.duration_since(state.last_prune) >= PRUNE_INTERVAL {
            state.buckets.retain(|_, bucket| {
                bucket.refill(now);
                bucket.tokens < BURST
            });
            state.last_prune = now;
        }
        let bucket = state.buckets.entry(address).or_insert(Bucket {
            tokens: BURST,
            refilled_at: now,
        });
        bucket.refill(now);
        if bucket.tokens < 1.0 {
            return false;
        }
        bucket.tokens -= 1.0;
        true
    }
}

#[cfg(test)]
mod tests {
    use std::{net::IpAddr, time::Duration};

    use tokio::time;

    use super::{BURST, RateLimiter};

    const CLIENT: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(203, 0, 113, 7));
    const OTHER: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(203, 0, 113, 8));

    #[tokio::test(start_paused = true)]
    async fn an_address_past_its_burst_waits_for_the_refill() {
        // Arrange
        let limiter = RateLimiter::new();
        for _ in 0..BURST as usize {
            assert!(limiter.allow(CLIENT));
        }

        // Act / Assert: the burst is spent, other addresses are not affected
        assert!(!limiter.allow(CLIENT));
        assert!(limiter.allow(OTHER));

        // Act
        time::advance(Duration::from_secs(1)).await;

        // Assert
        assert!(limiter.allow(CLIENT));
        assert!(!limiter.allow(CLIENT));
    }
}
