use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportStats {
    pub rtt: Duration,
    pub cwnd: u64,
    pub sent_packets: u64,
    pub lost_packets: u64,
    pub congestion_events: u64,
    pub sent_stream_data_blocked: u64,
    pub sent_data_blocked: u64,
    pub received_max_stream_data: u64,
}

impl From<quinn::ConnectionStats> for TransportStats {
    fn from(stats: quinn::ConnectionStats) -> Self {
        Self {
            rtt: stats.path.rtt,
            cwnd: stats.path.cwnd,
            sent_packets: stats.path.sent_packets,
            lost_packets: stats.path.lost_packets,
            congestion_events: stats.path.congestion_events,
            sent_stream_data_blocked: stats.frame_tx.stream_data_blocked,
            sent_data_blocked: stats.frame_tx.data_blocked,
            received_max_stream_data: stats.frame_rx.max_stream_data,
        }
    }
}
