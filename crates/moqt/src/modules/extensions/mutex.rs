use std::sync::{Mutex, MutexGuard};

/// Locks `mutex`, taking the guard even when a panic poisoned it. Use it only
/// for data that a panic cannot leave half-updated, such as a table changed by
/// single map or vector calls.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
