// moqt
pub(crate) type TrackNamespace = String;
pub(crate) type TrackNamespacePrefix = String;

// id
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) type SessionId = u64;

static LAST_SESSION_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) fn generate_session_id() -> SessionId {
    let epoch_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or_else(|_| LAST_SESSION_ID.load(Ordering::Relaxed).saturating_add(1));

    loop {
        let previous = LAST_SESSION_ID.load(Ordering::Relaxed);
        let candidate = epoch_nanos.max(previous.saturating_add(1));
        if LAST_SESSION_ID
            .compare_exchange(previous, candidate, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return candidate;
        }
    }
}

pub(crate) fn compose_session_track_key(session_id: SessionId, track_alias: u64) -> u128 {
    ((session_id as u128) << 64) | (track_alias as u128)
}

// parameter alias
pub(crate) type DeliveryTimeout = moqt::DeliveryTimeout;
pub(crate) type MaxCacheDuration = moqt::MaxCacheDuration;
