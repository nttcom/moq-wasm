use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, LazyLock, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use telemetry_report::{Scoped, current_scope, scoped};

/// The token of each session scope whose session has one.
static SCOPE_TOKENS: LazyLock<Mutex<HashMap<u64, Weak<Mutex<String>>>>> =
    LazyLock::new(Mutex::default);
static NEXT_SCOPE: AtomicU64 = AtomicU64::new(0);

/// The `telemetry_report` scope of one session: the records under it carry
/// the session's token as it is when they are emitted.
#[derive(Clone, Copy)]
pub(crate) struct SessionScope(u64);

impl SessionScope {
    pub(crate) fn new(token: Option<&Arc<Mutex<String>>>) -> Self {
        let scope = NEXT_SCOPE.fetch_add(1, Ordering::Relaxed);
        if let Some(token) = token {
            let mut tokens = SCOPE_TOKENS.lock().unwrap();
            tokens.retain(|_, token| token.strong_count() > 0);
            tokens.insert(scope, Arc::downgrade(token));
        }
        Self(scope)
    }

    pub(crate) fn scoped<F: Future>(self, future: F) -> Scoped<F> {
        scoped(self.0, future)
    }
}

/// The token of the session `scope` belongs to; `None` outside a session,
/// for a session without a token, or once the session is gone.
pub(super) fn token_of(scope: Option<u64>) -> Option<String> {
    let token = SCOPE_TOKENS.lock().unwrap().get(&scope?)?.upgrade()?;
    let token = token.lock().unwrap().clone();
    Some(token)
}

type SpawnedTask = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Polls each task `moqt::executor::spawn` starts in the scope it was
/// spawned in, so the session's tasks keep its token.
pub(super) fn carry_scope(future: SpawnedTask) -> SpawnedTask {
    match current_scope() {
        Some(scope) => Box::pin(scoped(scope, future)),
        None => future,
    }
}
