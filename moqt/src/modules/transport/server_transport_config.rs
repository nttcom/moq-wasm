use std::time::Duration;

use quinn::{TransportConfig, VarInt};

pub(crate) fn server_transport_config(keep_alive_sec: u64) -> TransportConfig {
    let mut transport_config = TransportConfig::default();
    transport_config.keep_alive_interval(Some(Duration::from_secs(keep_alive_sec)));
    transport_config.max_concurrent_uni_streams(100000u32.into());
    transport_config.packet_threshold(5);
    transport_config.stream_receive_window(VarInt::from_u32(1024 * 1024));
    transport_config
}
