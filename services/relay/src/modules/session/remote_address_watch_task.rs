use std::{net::SocketAddr, sync::Weak, time::Duration};

use tokio::time::MissedTickBehavior;
use tracing::{Instrument, Span};

use crate::modules::{domain::session_id::SessionId, session::Session};

/// quinn emits no event when the peer migrates (RFC 9000 §9), so the
/// remote address is polled.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Logs each change of a session's remote address, which is how a QUIC
/// connection migration or NAT rebinding of the peer shows on the relay.
pub(crate) struct RemoteAddressWatchTask {
    join_handle: tokio::task::JoinHandle<()>,
}

impl RemoteAddressWatchTask {
    pub(crate) fn run(
        session: Weak<dyn Session>,
        session_id: SessionId,
        session_span: Span,
    ) -> Self {
        let join_handle = tokio::spawn(
            async move {
                let mut last_remote_address = None;
                let mut ticker = tokio::time::interval(POLL_INTERVAL);
                ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                loop {
                    ticker.tick().await;
                    let Some(session) = session.upgrade() else {
                        return;
                    };
                    let Some(remote_address) = session.transport_addresses().remote else {
                        continue;
                    };
                    if let Some(previous_remote_address) =
                        replaced_address(&mut last_remote_address, remote_address)
                    {
                        tracing::info!(
                            session_id,
                            %previous_remote_address,
                            %remote_address,
                            "connection migrated"
                        );
                    }
                }
            }
            .instrument(session_span),
        );
        Self { join_handle }
    }
}

impl Drop for RemoteAddressWatchTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

fn replaced_address(last: &mut Option<SocketAddr>, current: SocketAddr) -> Option<SocketAddr> {
    let previous = last.replace(current)?;
    (previous != current).then_some(previous)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::replaced_address;

    fn address(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    #[test]
    fn the_first_or_an_unchanged_address_is_not_a_change() {
        // Arrange
        let mut last = None;

        // Act
        let first = replaced_address(&mut last, address("192.0.2.1:4433"));
        let unchanged = replaced_address(&mut last, address("192.0.2.1:4433"));

        // Assert
        assert_eq!((first, unchanged), (None, None));
    }

    #[test]
    fn each_change_returns_the_address_before_it() {
        // Arrange
        let mut last = Some(address("192.0.2.1:4433"));

        // Act
        let new_port = replaced_address(&mut last, address("192.0.2.1:1089"));
        let new_ip = replaced_address(&mut last, address("198.51.100.7:1089"));

        // Assert: a NAT rebinding (new port only) is a change too
        assert_eq!(
            (new_port, new_ip),
            (
                Some(address("192.0.2.1:4433")),
                Some(address("192.0.2.1:1089"))
            )
        );
    }
}
