use std::cell::Cell;

use crate::modules::transport::transport_stats::TransportStats;

/// Counts the bytes a browser WebTransport connection's streams and datagrams
/// carry, because Chrome has no `WebTransport.getStats()`. A write counts once
/// the browser accepts it, not once the peer acknowledges it; QUIC and HTTP/3
/// framing are not counted.
#[derive(Debug, Default)]
pub(crate) struct BrowserStats {
    sent_bytes: Cell<u64>,
    received_bytes: Cell<u64>,
}

impl BrowserStats {
    pub(crate) fn add_sent(&self, bytes: usize) {
        self.sent_bytes.set(self.sent_bytes.get() + bytes as u64);
    }

    pub(crate) fn add_received(&self, bytes: usize) {
        self.received_bytes
            .set(self.received_bytes.get() + bytes as u64);
    }

    pub(crate) fn snapshot(&self) -> TransportStats {
        TransportStats {
            sent_bytes: self.sent_bytes.get(),
            received_bytes: self.received_bytes.get(),
            ..TransportStats::default()
        }
    }
}
