use std::time::Duration;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransportStats {
    pub rtt: Duration,
    pub cwnd: u64,
    pub current_mtu: u16,
    pub sent_bytes: u64,
    pub sent_packets: u64,
    pub lost_packets: u64,
    pub congestion_events: u64,
    pub sent_stream_data_blocked: u64,
    pub sent_data_blocked: u64,
    pub received_bytes: u64,
    pub received_max_stream_data: u64,
    pub received_stream_data_blocked: u64,
    pub received_data_blocked: u64,
    pub received_reset_stream: u64,
    pub received_stop_sending: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl From<quinn::ConnectionStats> for TransportStats {
    fn from(stats: quinn::ConnectionStats) -> Self {
        Self {
            rtt: stats.path.rtt,
            cwnd: stats.path.cwnd,
            current_mtu: stats.path.current_mtu,
            sent_bytes: stats.udp_tx.bytes,
            sent_packets: stats.path.sent_packets,
            lost_packets: stats.path.lost_packets,
            congestion_events: stats.path.congestion_events,
            sent_stream_data_blocked: stats.frame_tx.stream_data_blocked,
            sent_data_blocked: stats.frame_tx.data_blocked,
            received_bytes: stats.udp_rx.bytes,
            received_max_stream_data: stats.frame_rx.max_stream_data,
            received_stream_data_blocked: stats.frame_rx.stream_data_blocked,
            received_data_blocked: stats.frame_rx.data_blocked,
            received_reset_stream: stats.frame_rx.reset_stream,
            received_stop_sending: stats.frame_rx.stop_sending,
        }
    }
}
