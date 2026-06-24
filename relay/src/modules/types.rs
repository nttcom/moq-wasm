// moqt
pub(crate) type TrackNamespace = String;
pub(crate) type TrackNamespacePrefix = String;

// id
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) type SessionId = u64;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TrackKey {
    pub(crate) track_namespace: String,
    pub(crate) track_name: String,
}

impl TrackKey {
    pub(crate) fn new(track_namespace: impl Into<String>, track_name: impl Into<String>) -> Self {
        Self {
            track_namespace: track_namespace.into(),
            track_name: track_name.into(),
        }
    }
}

impl std::fmt::Display for TrackKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.track_namespace, self.track_name)
    }
}

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
