use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Counts the requests of each client address in fixed windows and refuses
/// those beyond `max_requests` in a window. Counts live in this process only,
/// so with several instances each one limits on its own.
pub struct RateLimit {
    max_requests: u32,
    window: Duration,
    windows: Mutex<HashMap<IpAddr, Window>>,
}

struct Window {
    started_at: Instant,
    requests: u32,
}

impl RateLimit {
    pub fn new(max_requests: u32, window: Duration) -> Self {
        Self {
            max_requests,
            window,
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Counts one request from `client` and tells whether it is within the limit.
    pub(crate) fn allow(&self, client: IpAddr, now: Instant) -> bool {
        let mut windows = self
            .windows
            .lock()
            .expect("no holder of the rate limit windows panics while holding them");
        windows.retain(|_, window| now.duration_since(window.started_at) < self.window);
        let window = windows.entry(client).or_insert(Window {
            started_at: now,
            requests: 0,
        });
        if window.requests >= self.max_requests {
            return false;
        }
        window.requests += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    const CLIENT: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
    const OTHER_CLIENT: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2));

    #[test]
    fn requests_beyond_the_limit_in_a_window_are_refused() {
        // Arrange
        let limit = RateLimit::new(2, Duration::from_secs(60));
        let now = Instant::now();

        // Act
        let allowed: Vec<bool> = (0..3).map(|_| limit.allow(CLIENT, now)).collect();

        // Assert
        assert_eq!(allowed, [true, true, false]);
    }

    #[test]
    fn each_client_has_its_own_count() {
        // Arrange
        let limit = RateLimit::new(1, Duration::from_secs(60));
        let now = Instant::now();
        limit.allow(CLIENT, now);

        // Act / Assert
        assert!(limit.allow(OTHER_CLIENT, now));
    }

    #[test]
    fn a_new_window_starts_counting_again() {
        // Arrange
        let limit = RateLimit::new(1, Duration::from_secs(60));
        let now = Instant::now();
        limit.allow(CLIENT, now);

        // Act / Assert
        assert!(limit.allow(CLIENT, now + Duration::from_secs(60)));
    }
}
