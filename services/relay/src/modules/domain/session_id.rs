use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) type SessionId = u64;

static LAST_SESSION_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) fn generate_session_id() -> SessionId {
    let epoch_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or_else(|_| LAST_SESSION_ID.load(Ordering::Relaxed).saturating_add(1));

    let previous = LAST_SESSION_ID
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |previous| {
            Some(epoch_nanos.max(previous.saturating_add(1)))
        })
        .expect("the closure always returns Some");
    epoch_nanos.max(previous.saturating_add(1))
}
