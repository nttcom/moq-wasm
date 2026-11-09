use std::{
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use crate::modules::transport::transport_stats::TransportStats;

/// The counters a QMux session keeps for `TransportConnection::stats`. The
/// path figures quinn reports (cwnd, MTU, losses) belong to TCP and stay 0;
/// the RTT is the latest QX_PING round trip.
#[derive(Default)]
pub(crate) struct Stats {
    pub(crate) sent_bytes: AtomicU64,
    pub(crate) received_bytes: AtomicU64,
    pub(crate) sent_stream_data_blocked: AtomicU64,
    pub(crate) sent_data_blocked: AtomicU64,
    pub(crate) received_max_stream_data: AtomicU64,
    pub(crate) received_stream_data_blocked: AtomicU64,
    pub(crate) received_data_blocked: AtomicU64,
    pub(crate) received_reset_stream: AtomicU64,
    pub(crate) received_stop_sending: AtomicU64,
    rtt: Mutex<Duration>,
}

impl Stats {
    pub(crate) fn add(counter: &AtomicU64, amount: u64) {
        counter.fetch_add(amount, Ordering::Relaxed);
    }

    pub(crate) fn set_rtt(&self, rtt: Duration) {
        *self.rtt.lock().expect("stats lock") = rtt;
    }

    pub(crate) fn snapshot(&self) -> TransportStats {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        TransportStats {
            rtt: *self.rtt.lock().expect("stats lock"),
            sent_bytes: load(&self.sent_bytes),
            received_bytes: load(&self.received_bytes),
            sent_stream_data_blocked: load(&self.sent_stream_data_blocked),
            sent_data_blocked: load(&self.sent_data_blocked),
            received_max_stream_data: load(&self.received_max_stream_data),
            received_stream_data_blocked: load(&self.received_stream_data_blocked),
            received_data_blocked: load(&self.received_data_blocked),
            received_reset_stream: load(&self.received_reset_stream),
            received_stop_sending: load(&self.received_stop_sending),
            ..TransportStats::default()
        }
    }
}
