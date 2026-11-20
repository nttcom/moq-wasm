use std::time::Duration;

pub(crate) mod closed_spans;
pub(crate) mod directory_fixtures;
pub(crate) mod event_spans;
pub(crate) mod location_strategies;
pub(crate) mod mock_session;
pub(crate) mod relay_clients;
pub(crate) mod relay_harness;
pub(crate) mod stub_route_registry;

const WAIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Under a paused clock tokio advances time whenever the runtime is idle, so
/// the timeout fires while network I/O is still pending. Wrap only waits that
/// need no real I/O there.
pub(crate) async fn within<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(WAIT_TIMEOUT, future)
        .await
        .expect("the awaited future did not finish within the timeout")
}
