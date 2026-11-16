use async_trait::async_trait;

use crate::{
    SessionEvent,
    modules::executor::{MaybeSend, MaybeSync},
};

/// Takes some of the peer's requests before the session's `Origin` does, as
/// a media broadcast answers the tracks it publishes.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait EventHandler: MaybeSend + MaybeSync {
    /// Returns `event` when it is not taken, for the next handler. The event
    /// loop waits for the call, so do not wait on the peer beyond sending the
    /// answer.
    async fn handle(&self, event: SessionEvent) -> Option<SessionEvent>;
}

/// Names a handler `Session::add_handler` added, for `remove_handler`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HandlerId(pub(super) u64);
