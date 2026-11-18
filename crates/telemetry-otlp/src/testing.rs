use std::sync::LazyLock;

use tracing::{
    Dispatch, Subscriber,
    subscriber::{DefaultGuard, NoSubscriber},
};

/// Keeps `tracing-core` out of its single-dispatcher mode, where a callsite's
/// interest is cached from the default of whichever thread registers it first:
/// a concurrent test without a subscriber would cache `never` and disable the
/// capturing test's spans and events.
static SECOND_DISPATCHER: LazyLock<Dispatch> =
    LazyLock::new(|| Dispatch::new(NoSubscriber::default()));

/// `tracing::subscriber::set_default` for a test that captures spans or
/// events: the callsites it shares with concurrent tests stay enabled for it.
pub fn set_default(subscriber: impl Subscriber + Send + Sync + 'static) -> DefaultGuard {
    LazyLock::force(&SECOND_DISPATCHER);
    tracing::subscriber::set_default(subscriber)
}
